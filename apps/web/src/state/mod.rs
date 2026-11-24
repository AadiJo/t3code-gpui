//! Live server state for the views. Mirrors `apps/web/src/state`: the GPUI side of the client
//! runtime's stores.
//!
//! [`StateBridge`] is a GPUI global that starts the runtime's [`StateRuntime`] on the
//! connection runtime's tokio runtime. Each enabled environment gets a [`LiveEnvironment`]
//! entity that follows its shell and server config; [`shell::LiveShell`] joins every
//! environment into the sidebar's view model, and [`threads::ThreadViews`] holds the open
//! thread and the mapped view models of recently shown ones.
//!
//! Every runtime value arrives through a `tokio::sync::watch` receiver. The GPUI tasks here
//! await those receivers directly (tokio's sync primitives need no tokio runtime to await).
//! A watch keeps only the latest value, so however many stream events land between two
//! foreground wakes, the view sees one update; views then repaint at most once per frame.
//! (A timer between updates would cap them further, but GPUI timers do not fire under the
//! headless dispatcher the end-to-end checks run on.)

pub mod app_data;
pub mod composer;
pub mod palette;
pub mod shell;
pub mod sidebar_actions;
pub mod threads;

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, AsyncApp, Context, Entity, Global, Task};
use t3code_client_runtime::operations::EnvironmentCommands;
use t3code_client_runtime::state::client_store::ClientStore;
use t3code_client_runtime::state::drafts::Drafts;
use t3code_client_runtime::state::orchestration::Bodies;
use t3code_client_runtime::state::ui_state::UiState;
use t3code_client_runtime::state::{EnvironmentStore, ShellState, StateRuntime};
use t3code_contracts::{EnvironmentId, ServerConfig};
use tokio::sync::watch;

use crate::connection::ConnectionRuntime;

/// Follows `receiver` on the GPUI thread: calls `apply` with the current value at once and
/// then with the latest value after each change (changes between two wakes collapse into
/// one). Stops when the sender is dropped or `apply` returns `false`.
pub fn follow_watch<T: Clone + 'static>(
    mut receiver: watch::Receiver<T>,
    cx: &mut App,
    mut apply: impl FnMut(T, &mut AsyncApp) -> bool + 'static,
) -> Task<()> {
    cx.spawn(async move |cx| {
        let first = receiver.borrow_and_update().clone();
        if !apply(first, cx) {
            return;
        }
        loop {
            if receiver.changed().await.is_err() {
                return;
            }
            let value = receiver.borrow_and_update().clone();
            if !apply(value, cx) {
                return;
            }
        }
    })
}

/// One enabled environment: its store, commands and loaders, with the shell and config the
/// views last saw. Notifies when either changes.
pub struct LiveEnvironment {
    pub store: EnvironmentStore,
    pub commands: EnvironmentCommands,
    pub bodies: Bodies,
    pub shell: Arc<ShellState>,
    pub config: Option<Arc<ServerConfig>>,
    _tasks: Vec<Task<()>>,
}

impl LiveEnvironment {
    fn new(store: EnvironmentStore, cx: &mut Context<Self>) -> Self {
        let shell = store.shell().current();
        let config = store.config().borrow().clone();
        let weak = cx.weak_entity();
        let shell_task = follow_watch(store.shell().subscribe(), cx, {
            let weak = weak.clone();
            move |shell, cx| {
                weak.update(cx, |this, cx| {
                    if !Arc::ptr_eq(&this.shell, &shell) {
                        this.shell = shell;
                        cx.notify();
                    }
                })
                .is_ok()
            }
        });
        let config_task = follow_watch(store.config(), cx, move |config, cx| {
            weak.update(cx, |this, cx| {
                let same = match (&this.config, &config) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    this.config = config;
                    cx.notify();
                }
            })
            .is_ok()
        });
        Self {
            commands: EnvironmentCommands::new(store.clone()),
            bodies: Bodies::new(store.clone()),
            store,
            shell,
            config,
            _tasks: vec![shell_task, config_task],
        }
    }

    pub fn environment_id(&self) -> &EnvironmentId {
        self.store.environment_id()
    }

    /// Whether the environment's config has arrived (live or cached); snoozed and settled
    /// sections wait for it.
    pub fn config_loaded(&self) -> bool {
        self.config.is_some()
    }
}

/// The global. See the module docs.
pub struct StateBridge {
    runtime: Arc<StateRuntime>,
    environments: Entity<Environments>,
    shell: Entity<shell::LiveShell>,
    threads: Entity<threads::ThreadViews>,
    drafts: Rc<ClientStore<Drafts>>,
    ui_state: Rc<ClientStore<UiState>>,
    tokio: tokio::runtime::Handle,
}

impl Global for StateBridge {}

/// Every enabled environment's entity, in catalog order. Notifies when the set changes.
#[derive(Default)]
pub struct Environments {
    pub list: Vec<Entity<LiveEnvironment>>,
    _task: Option<Task<()>>,
}

impl Environments {
    /// The entity of one environment.
    pub fn get(&self, id: &EnvironmentId, cx: &App) -> Option<Entity<LiveEnvironment>> {
        self.list.iter().find(|env| env.read(cx).environment_id() == id).cloned()
    }
}

impl StateBridge {
    /// Starts the runtime over the connection runtime's registry. Call once after
    /// [`ConnectionRuntime::init`]. Stores open from the disk cache synchronously, so the
    /// first frame already shows cached rows.
    pub fn init(cx: &mut App) {
        let connection = ConnectionRuntime::global(cx);
        let tokio = connection.tokio().clone();
        let data_dir = connection.data_dir().to_owned();
        let (runtime, drafts, ui_state) = {
            let _guard = tokio.enter();
            let runtime = StateRuntime::start(connection.registry().clone(), Arc::clone(connection.auth()), &data_dir);
            (runtime, ClientStore::<Drafts>::open(&data_dir), ClientStore::<UiState>::open(&data_dir))
        };
        let runtime = Arc::new(runtime);
        let environments = cx.new(|_| Environments::default());
        let drafts = Rc::new(drafts);
        let ui_state = Rc::new(ui_state);
        let shell = cx.new(|cx| shell::LiveShell::new(environments.clone(), Rc::clone(&drafts), Rc::clone(&ui_state), cx));
        let threads = cx.new(|_| threads::ThreadViews::default());
        cx.set_global(Self { runtime: Arc::clone(&runtime), environments: environments.clone(), shell, threads, drafts, ui_state, tokio });
        // The registry publishes entries before it starts their supervisors; the runtime's
        // first store list can lag a tick behind. Open the stores that exist now at once.
        let task = follow_watch(runtime.stores(), cx, {
            let environments = environments.downgrade();
            move |stores, cx| {
                environments
                    .update(cx, |this, cx| {
                        let mut previous = std::mem::take(&mut this.list);
                        this.list = stores
                            .iter()
                            .map(|store| {
                                match previous.iter().position(|env| env.read(cx).environment_id() == store.environment_id()) {
                                    Some(ix) => previous.swap_remove(ix),
                                    None => cx.new(|cx| LiveEnvironment::new(store.clone(), cx)),
                                }
                            })
                            .collect();
                        cx.notify();
                    })
                    .is_ok()
            }
        });
        environments.update(cx, |this, _| this._task = Some(task));
    }

    pub fn is_running(cx: &App) -> bool {
        cx.has_global::<Self>()
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn runtime(&self) -> &Arc<StateRuntime> {
        &self.runtime
    }

    pub fn environments(cx: &App) -> Entity<Environments> {
        cx.global::<Self>().environments.clone()
    }

    pub fn shell(cx: &App) -> Entity<shell::LiveShell> {
        cx.global::<Self>().shell.clone()
    }

    pub fn threads(cx: &App) -> Entity<threads::ThreadViews> {
        cx.global::<Self>().threads.clone()
    }

    pub fn drafts(cx: &App) -> Rc<ClientStore<Drafts>> {
        Rc::clone(&cx.global::<Self>().drafts)
    }

    pub fn ui_state(cx: &App) -> Rc<ClientStore<UiState>> {
        Rc::clone(&cx.global::<Self>().ui_state)
    }

    /// The live entity of one environment.
    pub fn environment(id: &EnvironmentId, cx: &App) -> Option<Entity<LiveEnvironment>> {
        Self::environments(cx).read(cx).get(id, cx)
    }

    /// Runs `future` on tokio (RPC calls need its reactor) and resolves on GPUI.
    pub fn spawn<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static, cx: &App) -> Task<Option<T>> {
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.global::<Self>().tokio.spawn(async move {
            _ = sender.send(future.await);
        });
        cx.background_executor().spawn(async move { receiver.await.ok() })
    }
}

/// A shared, mutable value the bridge's sources read from GPUI callbacks.
pub type Shared<T> = Rc<RefCell<T>>;
