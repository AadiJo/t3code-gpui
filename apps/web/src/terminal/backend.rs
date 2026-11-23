//! The terminal's only link to the outside world. A backend delivers bytes in
//! (server output, attach snapshots, exit) and receives input, resizes and
//! control requests out. The RPC wiring (`terminal.attach`, `terminal.write`,
//! `terminal.resize`, `terminal.clear`, `terminal.close`) implements this
//! trait; the view never knows which backend it is talking to.

use futures::channel::mpsc;

/// How many inbound events can wait before `send` on the sink blocks. The
/// server keeps at most 8 chunks or 64 KiB in flight per terminal stream
/// (`01-wire-protocol.md` 3.6), so 16 leaves room for control events.
pub const INBOUND_CAPACITY: usize = 16;

/// Grid dimensions in cells, as the server's `cols`/`rows` fields use them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
}

impl GridSize {
    /// Clamps to the server's accepted range (cols 1..=1000, rows 1..=500).
    pub fn clamped(cols: usize, rows: usize) -> Self {
        Self {
            cols: cols.clamp(1, 1000) as u16,
            rows: rows.clamp(1, 500) as u16,
        }
    }
}

/// Something a backend delivers to the terminal.
#[derive(Clone, Debug)]
pub enum TerminalInbound {
    /// Replace everything on screen with this history: an attach snapshot or a reconnect.
    Reset(Vec<u8>),
    /// Program output to append.
    Output(Vec<u8>),
    /// The server cleared the session's history.
    Cleared,
    /// The process exited.
    Exited { code: Option<i64>, signal: Option<i64> },
    /// The session was closed.
    Closed,
    /// The backend failed; the terminal prints it as a `[terminal]` line.
    Error(String),
}

/// Whether bytes came from the user or are the terminal answering a query.
/// Maps to `terminal.write`'s `reply` flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Typed,
    Reply,
}

/// Something the terminal asks its backend to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalOutbound {
    /// Bytes for the process: keystrokes, pastes, mouse reports, or query replies.
    Input { bytes: Vec<u8>, kind: InputKind },
    /// The grid settled at a new size (debounced by the view).
    Resize(GridSize),
    /// Clear the session's server-side history.
    Clear,
    /// Close the session.
    Close,
}

/// The sending half a backend pushes inbound events into. Await `send` before
/// acknowledging a stream chunk: a full channel is the backpressure signal.
pub type TerminalSink = mpsc::Sender<TerminalInbound>;

/// The receiving half the view drains.
pub type TerminalSource = mpsc::Receiver<TerminalInbound>;

/// A transport for one terminal session.
pub trait TerminalBackend: 'static {
    /// Starts the session at `size` and delivers its events into `sink` until it ends.
    /// Called once, by the view that owns this backend.
    fn connect(&mut self, sink: TerminalSink, size: GridSize);

    /// Handles one outbound request. Must not block.
    fn send(&mut self, message: TerminalOutbound);
}

/// A bounded inbound channel sized for the server's flow-control window.
pub fn inbound_channel() -> (TerminalSink, TerminalSource) {
    mpsc::channel(INBOUND_CAPACITY)
}

/// Produces a recording for the grid size the terminal opened at.
pub type Recording = Box<dyn FnOnce(GridSize) -> Vec<Vec<u8>>>;

/// A backend that replays recorded output once and keeps what is sent to it.
/// Used by scenes and benchmarks.
pub struct RecordedBackend {
    recording: Option<Recording>,
    /// Everything the terminal sent out, for inspection.
    pub sent: Vec<TerminalOutbound>,
}

impl RecordedBackend {
    /// Replays fixed chunks.
    pub fn new(chunks: Vec<Vec<u8>>) -> Self {
        Self::sized(move |_| chunks)
    }

    /// Replays chunks drawn for the opening size, like a full-screen program would.
    pub fn sized(recording: impl FnOnce(GridSize) -> Vec<Vec<u8>> + 'static) -> Self {
        Self { recording: Some(Box::new(recording)), sent: Vec::new() }
    }
}

impl TerminalBackend for RecordedBackend {
    fn connect(&mut self, mut sink: TerminalSink, size: GridSize) {
        let Some(recording) = self.recording.take() else { return };
        let mut chunks = recording(size).into_iter();
        // Whatever fits is queued now, so the first drained batch shows it.
        for chunk in chunks.by_ref() {
            if let Err(error) = sink.try_send(TerminalInbound::Output(chunk)) {
                let rest: Vec<_> = std::iter::once(error.into_inner()).chain(chunks.map(TerminalInbound::Output)).collect();
                std::thread::spawn(move || {
                    futures::executor::block_on(async move {
                        use futures::SinkExt as _;
                        for event in rest {
                            if sink.send(event).await.is_err() {
                                break;
                            }
                        }
                    });
                });
                return;
            }
        }
    }

    fn send(&mut self, message: TerminalOutbound) {
        self.sent.push(message);
    }
}
