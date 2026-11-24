//! Terminals on the server (`client-runtime/state/terminal.ts`): [`TerminalBackend`] over
//! `terminal.attach` (a stream that opens the session or resumes it), `terminal.write`,
//! `terminal.resize`, `terminal.clear` and `terminal.close`. Each output chunk is acked only
//! after the terminal's inbound queue took it, so a terminal that stops drawing stops the
//! server's stream.

use std::rc::Rc;

use base64::Engine as _;
use futures::SinkExt as _;
use futures::StreamExt as _;
use gpui_kit::App;
use t3code_contracts::rpc::methods::{TerminalAttach, TerminalClear, TerminalClose, TerminalResize, TerminalWrite};
use t3code_contracts::{
    EnvironmentId, TerminalAttachInput, TerminalAttachStreamEvent, TerminalClearPayload, TerminalCloseInput, TerminalResizeInput,
    TerminalWriteInput,
};
use tokio::sync::mpsc;

use super::rpc::RpcLink;
use crate::components::right_panel_tabs::TerminalBackends;
use crate::components::thread_terminal_drawer::TerminalLaunch;
use crate::terminal::backend::{GridSize, InputKind, TerminalBackend, TerminalInbound, TerminalOutbound, TerminalSink};

/// A server terminal session for one thread's terminal id.
pub struct RpcTerminalBackend {
    link: Option<RpcLink>,
    thread_id: String,
    terminal_id: String,
    cwd: String,
    worktree_path: Option<String>,
    /// A command to run once the session is attached (project scripts).
    command: Option<String>,
    outbound: Option<mpsc::UnboundedSender<TerminalOutbound>>,
}

impl RpcTerminalBackend {
    /// A backend for `launch`, whose thread key is `environmentId:threadId`.
    pub fn new(launch: &TerminalLaunch, command: Option<String>, cx: &App) -> Self {
        let (environment, thread) = launch.thread.0.split_once(':').unwrap_or_default();
        let link = (!thread.is_empty() && environment != "draft").then(|| RpcLink::new(&EnvironmentId::new(environment), cx)).flatten();
        Self {
            link,
            thread_id: thread.to_owned(),
            terminal_id: launch.terminal_id.to_string(),
            cwd: launch.cwd.to_string(),
            worktree_path: launch.worktree_path.as_ref().map(|p| p.to_string()),
            command,
            outbound: None,
        }
    }
}

fn inbound(event: TerminalAttachStreamEvent) -> Option<TerminalInbound> {
    Some(match event {
        TerminalAttachStreamEvent::Snapshot(e) => TerminalInbound::Reset(e.snapshot.history.into_bytes()),
        TerminalAttachStreamEvent::Restarted(e) => TerminalInbound::Reset(e.snapshot.history.into_bytes()),
        TerminalAttachStreamEvent::Output(e) => TerminalInbound::Output(e.data.into_bytes()),
        TerminalAttachStreamEvent::Exited(e) => TerminalInbound::Exited { code: e.exit_code, signal: e.exit_signal },
        TerminalAttachStreamEvent::Closed(_) => TerminalInbound::Closed,
        TerminalAttachStreamEvent::Error(e) => TerminalInbound::Error(e.message),
        TerminalAttachStreamEvent::Cleared(_) => TerminalInbound::Cleared,
        TerminalAttachStreamEvent::Activity(_) | TerminalAttachStreamEvent::Unknown(_) => return None,
    })
}

impl TerminalBackend for RpcTerminalBackend {
    fn connect(&mut self, mut sink: TerminalSink, size: GridSize) {
        let Some(link) = self.link.clone() else {
            _ = sink.try_send(TerminalInbound::Error("This environment is not connected.".into()));
            return;
        };
        let (sender, mut requests) = mpsc::unbounded_channel();
        if let Some(command) = self.command.take() {
            _ = sender.send(TerminalOutbound::Input { bytes: format!("{command}\r").into_bytes(), kind: InputKind::Typed });
        }
        self.outbound = Some(sender);
        let (thread_id, terminal_id) = (self.thread_id.clone(), self.terminal_id.clone());
        let attach = TerminalAttachInput {
            thread_id: thread_id.clone(),
            terminal_id: terminal_id.clone(),
            cwd: Some(self.cwd.clone()).filter(|cwd| !cwd.is_empty()),
            worktree_path: self.worktree_path.clone(),
            cols: Some(size.cols.into()),
            rows: Some(size.rows.into()),
            env: None,
            provider_instance_id: None,
            stop_on_settle: None,
            restart_if_not_running: Some(true),
            resume: None,
            max_history_bytes: None,
        };
        let tokio = link.tokio.clone();
        tokio.spawn(async move {
            let session = match link.session().await {
                Ok(session) => session,
                Err(error) => {
                    _ = sink.send(TerminalInbound::Error(error.to_string())).await;
                    return;
                }
            };
            let mut stream = session.client.subscribe_manual_ack::<TerminalAttach>(attach);
            // Requests wait for the attach to open the session, then run in order.
            let mut requests = Some(requests);
            let mut writer: Option<tokio::task::JoinHandle<()>> = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(events) => {
                        for event in events.into_iter().filter_map(inbound) {
                            if sink.send(event).await.is_err() {
                                if let Some(writer) = &writer {
                                    writer.abort();
                                }
                                return;
                            }
                        }
                        stream.ack();
                        if let Some(requests) = requests.take() {
                            writer = Some(tokio::spawn(run_requests(session.clone(), thread_id.clone(), terminal_id.clone(), requests)));
                        }
                    }
                    Err(error) => {
                        _ = sink.send(TerminalInbound::Error(error.to_string())).await;
                        break;
                    }
                }
            }
        });
    }

    fn send(&mut self, message: TerminalOutbound) {
        if let Some(outbound) = &self.outbound {
            _ = outbound.send(message);
        }
    }
}

/// Runs the terminal's writes, resizes, clears and close in order.
async fn run_requests(
    session: std::sync::Arc<t3code_client_runtime::rpc::RpcSession>,
    ids_thread: String,
    ids_terminal: String,
    mut requests: mpsc::UnboundedReceiver<TerminalOutbound>,
) {
    let client = &session.client;
    while let Some(request) = requests.recv().await {
        let (thread_id, terminal_id) = (ids_thread.clone(), ids_terminal.clone());
        let result = match request {
            TerminalOutbound::Input { bytes, kind } => {
                let reply = (kind == InputKind::Reply).then_some(true);
                let (data, binary) = match String::from_utf8(bytes) {
                    Ok(text) => (text, None),
                    Err(error) => (base64::engine::general_purpose::STANDARD.encode(error.into_bytes()), Some(true)),
                };
                client.call::<TerminalWrite>(TerminalWriteInput { thread_id, terminal_id, data, binary, reply }).await.err().map(|e| e.to_string())
            }
            TerminalOutbound::Resize(size) => client
                .call::<TerminalResize>(TerminalResizeInput { thread_id, terminal_id, cols: size.cols.into(), rows: size.rows.into() })
                .await
                .err()
                .map(|e| e.to_string()),
            TerminalOutbound::Clear => {
                client.call::<TerminalClear>(TerminalClearPayload { thread_id, terminal_id }).await.err().map(|e| e.to_string())
            }
            TerminalOutbound::Close => {
                let input = TerminalCloseInput { thread_id, terminal_id: Some(terminal_id), delete_history: None };
                _ = client.call::<TerminalClose>(input).await;
                return;
            }
        };
        if let Some(error) = result {
            tracing::warn!(%error, "terminal request failed");
        }
    }
}

/// Shells and project scripts as server terminals.
pub fn backends(_: &mut App) -> TerminalBackends {
    TerminalBackends {
        shell: Rc::new(|launch: &TerminalLaunch, cx: &mut App| Box::new(RpcTerminalBackend::new(launch, None, cx))),
        script: Rc::new(|launch: &TerminalLaunch, command: &str, cx: &mut App| {
            Box::new(RpcTerminalBackend::new(launch, Some(command.to_owned()), cx))
        }),
    }
}
