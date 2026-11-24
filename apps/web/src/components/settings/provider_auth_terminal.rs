//! `ProviderAuthTerminal`: a provider login that runs in a terminal on the
//! server, shown in a real [`TerminalSurface`]. The interaction's output
//! (a tail starting at `outputOffset`) is fed in as it grows; keystrokes,
//! terminal replies and resizes go back with `provider.auth.respond`.

use std::cell::RefCell;
use std::rc::Rc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{AppContext as _, Context, Entity, Task, Window};
use t3code_contracts::{
    ProviderAuthInteractionTerminal, ProviderAuthRespondInput, ProviderAuthResponse, ProviderAuthResponseTerminal,
    ProviderAuthResponseTerminalSize, ProviderInstanceId,
};

use super::provider_settings_panel::ProviderSettingsPanel;
use crate::terminal::backend::TerminalSink;
use crate::terminal::{GridSize, TerminalBackend, TerminalInbound, TerminalOutbound, TerminalSurface};

/// The surface's inbound sink once it connects, and output that arrived before.
#[derive(Default)]
struct Inbound {
    sink: Option<TerminalSink>,
    pending: Vec<u8>,
}

impl Inbound {
    fn push(&mut self, bytes: Vec<u8>) {
        match &mut self.sink {
            Some(sink) => {
                // A full channel drops this chunk's view only; the interaction keeps the full log.
                let _ = sink.try_send(TerminalInbound::Output(bytes));
            }
            None => self.pending.extend(bytes),
        }
    }
}

/// One sign-in terminal, kept by the panel per instance while its flow runs.
pub struct AuthTerminal {
    pub surface: Entity<TerminalSurface>,
    inbound: Rc<RefCell<Inbound>>,
    /// Bytes of the interaction's output already written, counted from its start.
    written: u64,
    flow_id: String,
    interaction_id: String,
    _pump: Task<()>,
}

/// Hands outbound requests to the panel and keeps the sink the surface opens.
struct AuthBackend {
    inbound: Rc<RefCell<Inbound>>,
    outbound: mpsc::UnboundedSender<TerminalOutbound>,
}

impl TerminalBackend for AuthBackend {
    fn connect(&mut self, sink: TerminalSink, size: GridSize) {
        let mut inbound = self.inbound.borrow_mut();
        inbound.sink = Some(sink);
        let pending = std::mem::take(&mut inbound.pending);
        if !pending.is_empty() {
            inbound.push(pending);
        }
        let _ = self.outbound.unbounded_send(TerminalOutbound::Resize(size));
    }

    fn send(&mut self, message: TerminalOutbound) {
        let _ = self.outbound.unbounded_send(message);
    }
}

impl ProviderSettingsPanel {
    /// The terminal for `instance`'s current terminal interaction, created on
    /// first use and fed any output it has not shown yet.
    pub(super) fn auth_terminal(
        &mut self,
        instance: &str,
        flow_id: &str,
        interaction: &ProviderAuthInteractionTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalSurface> {
        let stale = self
            .auth_terminals
            .get(instance)
            .is_none_or(|t| t.interaction_id != interaction.id || t.flow_id != flow_id);
        if stale {
            let inbound = Rc::new(RefCell::new(Inbound::default()));
            let (tx, mut rx) = mpsc::unbounded();
            let backend = AuthBackend { inbound: inbound.clone(), outbound: tx };
            let surface = cx.new(|cx| TerminalSurface::new(Box::new(backend), window, cx));
            let (instance_id, flow, interaction_id) = (instance.to_owned(), flow_id.to_owned(), interaction.id.clone());
            let pump = cx.spawn(async move |this, cx| {
                while let Some(message) = rx.next().await {
                    let response = match message {
                        TerminalOutbound::Input { bytes, .. } => match String::from_utf8(bytes) {
                            Ok(data) => ProviderAuthResponseTerminal { data, binary: None, size: None },
                            Err(e) => ProviderAuthResponseTerminal {
                                data: e.into_bytes().iter().map(|b| char::from(*b)).collect(),
                                binary: Some(true),
                                size: None,
                            },
                        },
                        TerminalOutbound::Resize(size) => ProviderAuthResponseTerminal {
                            data: String::new(),
                            binary: None,
                            size: Some(ProviderAuthResponseTerminalSize { cols: size.cols.into(), rows: size.rows.into() }),
                        },
                        TerminalOutbound::Clear | TerminalOutbound::Close => continue,
                    };
                    let input = ProviderAuthRespondInput {
                        instance_id: ProviderInstanceId::new(instance_id.as_str()),
                        flow_id: flow.clone(),
                        interaction_id: interaction_id.clone(),
                        response: ProviderAuthResponse::Terminal(response),
                    };
                    let id = instance_id.clone();
                    if this.update(cx, |this, cx| this.run_auth(id, move |s| s.respond_provider_auth(input), cx)).is_err() {
                        return;
                    }
                }
            });
            self.auth_terminals.insert(
                instance.to_owned(),
                AuthTerminal { surface, inbound, written: 0, flow_id: flow_id.to_owned(), interaction_id: interaction.id.clone(), _pump: pump },
            );
        }
        let terminal = self.auth_terminals.get_mut(instance).expect("inserted above");
        let start = interaction.output_offset.unwrap_or(0);
        let end = start + interaction.output.len() as u64;
        if end > terminal.written {
            let from = terminal.written.saturating_sub(start) as usize;
            let bytes = interaction.output.as_bytes().get(from..).unwrap_or_default().to_vec();
            terminal.inbound.borrow_mut().push(bytes);
            terminal.written = end;
        }
        terminal.surface.clone()
    }
}
