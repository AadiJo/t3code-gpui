//! A backend that runs a local shell in a PTY. For tests, benchmarks and
//! scripted screenshots only: the app talks to remote servers through RPC.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, Options, Pty, Shell};
use futures::SinkExt as _;

use super::backend::{GridSize, TerminalBackend, TerminalInbound, TerminalOutbound, TerminalSink};

/// A shell (or any program) in a local pseudo-terminal.
pub struct LocalPty {
    options: Options,
    pty: Option<Pty>,
    writer: Option<File>,
}

impl LocalPty {
    /// Runs `program args` in `cwd`, with `TERM=xterm-256color`.
    pub fn new(program: &str, args: &[&str], cwd: Option<PathBuf>) -> Self {
        let env = HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
        ]);
        Self {
            options: Options {
                shell: Some(Shell::new(program.to_string(), args.iter().map(|arg| arg.to_string()).collect())),
                working_directory: cwd,
                drain_on_exit: true,
                env,
            },
            pty: None,
            writer: None,
        }
    }

    /// Adds an environment variable for the child.
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.options.env.insert(key.to_string(), value.to_string());
        self
    }

    /// The child's process id, once connected.
    pub fn pid(&self) -> Option<u32> {
        self.pty.as_ref().map(|pty| pty.child().id())
    }
}

fn window_size(size: GridSize) -> WindowSize {
    WindowSize { num_lines: size.rows, num_cols: size.cols, cell_width: 8, cell_height: 16 }
}

impl TerminalBackend for LocalPty {
    fn connect(&mut self, mut sink: TerminalSink, size: GridSize) {
        let pty = match tty::new(&self.options, window_size(size), 0) {
            Ok(pty) => pty,
            Err(error) => {
                let _ = sink.try_send(TerminalInbound::Error(format!("failed to start PTY: {error}")));
                return;
            }
        };
        let file = pty.file();
        // alacritty opens the master non-blocking for its poller; this backend reads on a thread.
        unsafe {
            let fd = file.as_raw_fd();
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL, 0) & !libc::O_NONBLOCK);
        }
        let (Ok(mut reader), Ok(writer)) = (file.try_clone(), file.try_clone()) else {
            let _ = sink.try_send(TerminalInbound::Error("failed to clone PTY".into()));
            return;
        };
        self.writer = Some(writer);
        self.pty = Some(pty);
        std::thread::spawn(move || {
            futures::executor::block_on(async move {
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if sink.send(TerminalInbound::Output(buffer[..read].to_vec())).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = sink.send(TerminalInbound::Exited { code: None, signal: None }).await;
            });
        });
    }

    fn send(&mut self, message: TerminalOutbound) {
        match message {
            TerminalOutbound::Input { bytes, .. } => {
                if let Some(writer) = self.writer.as_mut() {
                    let _ = writer.write_all(&bytes);
                }
            }
            TerminalOutbound::Resize(size) => {
                if let Some(pty) = self.pty.as_mut() {
                    pty.on_resize(window_size(size));
                }
            }
            TerminalOutbound::Clear => {}
            TerminalOutbound::Close => {
                self.writer = None;
                // Dropping the PTY hangs up the child.
                self.pty = None;
            }
        }
    }
}
