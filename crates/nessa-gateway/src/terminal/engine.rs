//! The Ghostty engine: one thread per terminal owns a pty-runtime session in
//! raw mode and a Ghostty terminal model fed by it, and draws each attached
//! view from that one model. Modelled on pty-runtime's reference server
//! (`client/server/src/session.rs`), with one model and many views.
//!
//! The runtime's session and terminal types are synchronous and polled, so
//! they stay on this thread and never meet the async executor.
use super::wire::{self, Sent, ServerMessage, StyleTable, VERSION};
use super::{
    EngineCommand, EngineEvents, RunningTerminal, StartSpec, TerminalEngine, TerminalError,
    TerminalKind, TerminalSignal, ViewKey,
};
use crate::timer::clock;
use pty_runtime::{
    ports::{ITerminal, ITerminalFactory, ProcessOperation},
    terminal::{TerminalConfig, TerminalPalette},
    AttachPosition, CommandSpec, EnvironmentPolicy, ExitStatus, OutputEvent, ReplayPage, Runtime,
    SessionId, SessionOptions, TerminalSize, WriteOutcome,
};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::future::Future;
use std::pin::Pin;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::mpsc;

/// The interval between rendered frames. Output is fed as fast as it comes
/// but projected on this cadence, so `yes` does not make a frame per read.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// The largest message text sent in one event, under the product frame's
/// 64 KiB payload with room for the envelope. A frame with more changed rows
/// is sent as several frames with the same cursor.
const MESSAGE_BUDGET: usize = 48 * 1024;
/// How long a hung-up program has before it is killed and forgotten.
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// Starts terminals on the runtime built at process start.
pub struct GhosttyEngine {
    runtime: Arc<Runtime>,
}

impl GhosttyEngine {
    /// `runtime` must have been built on the main thread before the async
    /// runtime started (pty-runtime stages its helper image by forking).
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }
}

struct Thread(Mutex<std_mpsc::Sender<EngineCommand>>);

impl RunningTerminal for Thread {
    fn send(&self, command: EngineCommand) {
        if let Ok(sender) = self.0.lock() {
            let _ = sender.send(command);
        }
    }
}

impl TerminalEngine for GhosttyEngine {
    fn start(
        &self,
        id: &str,
        spec: &StartSpec,
        events: Arc<dyn EngineEvents>,
    ) -> Result<Box<dyn RunningTerminal>, TerminalError> {
        let size = TerminalSize::new(spec.size.cols, spec.size.rows)
            .map_err(|_| TerminalError::Invalid)?;
        // A login shell reads the person's profile, so PATH and the rest are
        // theirs, not the gateway's. TERM says which sequences it may send.
        let mut arguments: Vec<OsString> = vec!["-l".into()];
        if let TerminalKind::Run { command } = &spec.kind {
            arguments.extend(["-i".into(), "-c".into(), command.into()]);
        }
        let command = CommandSpec::new(spec.shell.clone(), spec.folder.clone(), arguments)
            .and_then(|command| {
                command.with_environment(
                    EnvironmentPolicy::Inherit,
                    vec!["NO_COLOR".into(), "FORCE_COLOR".into()],
                    vec![
                        ("TERM".into(), "xterm-256color".into()),
                        ("COLORTERM".into(), "truecolor".into()),
                        ("TERM_PROGRAM".into(), "Nessa".into()),
                    ],
                )
            })
            .map_err(|error| TerminalError::Spawn(format!("{error:?}")))?;
        let session_id = SessionId::new(id.to_owned()).map_err(|_| TerminalError::Invalid)?;
        let mut options = SessionOptions::raw(size);
        options.replay_bytes = 1024 * 1024;
        // A write larger than this is refused, so input goes a chunk at a time.
        let input_chunk = options.process.input_chunk;
        let session = self
            .runtime
            .spawn(session_id.clone(), &command, options)
            .map_err(|error| TerminalError::Spawn(format!("{error:?}")))?;
        let observer = match session.attach(AttachPosition::Oldest) {
            Ok(observer) => observer,
            Err(error) => {
                let _ = session.cancel();
                return Err(TerminalError::Spawn(format!("{error:?}")));
            }
        };
        let factory = pty_runtime_infrastructure::terminal::GhosttyTerminalFactory;
        let terminal = match factory.create(terminal_config(size)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = session.cancel();
                return Err(TerminalError::Spawn(format!("{error:?}")));
            }
        };
        let (sender, commands) = std_mpsc::channel();
        let runtime = Arc::clone(&self.runtime);
        let terminal_id = id.to_owned();
        std::thread::Builder::new()
            .name(format!("terminal-{id}"))
            .spawn(move || {
                let mut run = Run {
                    id: terminal_id,
                    terminal,
                    views: HashMap::new(),
                    palette: blank_palette(),
                    exit: None,
                };
                run.drive(&session, observer, commands, &*events, input_chunk);
                let _ = session.cancel();
                if let Ok(mut completion) = session.wait() {
                    let deadline = clock::os::now() + CLOSE_GRACE;
                    while clock::os::now() < deadline && poll(&mut completion).is_none() {
                        clock::os::sleep(Duration::from_millis(5));
                    }
                }
                let _ = runtime.forget(&session_id);
            })
            .map_err(|error| TerminalError::Spawn(error.to_string()))?;
        Ok(Box::new(Thread(Mutex::new(sender))))
    }
}

fn poll<F: Future + Unpin>(future: &mut F) -> Option<F::Output> {
    match Pin::new(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => Some(value),
        Poll::Pending => None,
    }
}

fn terminal_config(size: TerminalSize) -> TerminalConfig {
    TerminalConfig {
        size,
        history_bytes: 16 * 1024 * 1024,
        continuation_bytes: 64 * 1024,
        reply_bytes: 4096,
        checkpoint_bytes: 8 * 1024 * 1024,
        feed_bytes: 4096,
        native_bytes: 32 * 1024 * 1024,
        view_bytes: 1024 * 1024,
    }
}

fn blank_palette() -> TerminalPalette {
    TerminalPalette {
        foreground: None,
        background: None,
        cursor: None,
        indexed: [[0; 3]; 256],
    }
}

/// What one view has been sent.
struct View {
    out: mpsc::Sender<TerminalSignal>,
    sent: Sent,
    styles: StyleTable,
    /// Sent nothing yet, or lost a message: draw it whole next time.
    redraw: bool,
    /// Whether it has been told the program ended.
    exit_told: bool,
}

struct Run {
    id: String,
    terminal: Box<dyn ITerminal>,
    views: HashMap<ViewKey, View>,
    palette: TerminalPalette,
    /// Set once the program ended; `Some(None)` when a signal ended it.
    exit: Option<Option<i32>>,
}

impl Run {
    fn drive(
        &mut self,
        session: &pty_runtime::Session,
        mut observer: pty_runtime::Attachment,
        commands: std_mpsc::Receiver<EngineCommand>,
        events: &dyn EngineEvents,
        input_chunk: usize,
    ) {
        let mut pending_input: VecDeque<u8> = VecDeque::new();
        let mut writing: Option<(ProcessOperation<WriteOutcome>, Vec<u8>)> = None;
        let mut resizing = None;
        let mut generation = pty_runtime::terminal::ControlGeneration::INITIAL;
        let mut next_frame = clock::os::now();
        let mut dirty = true;
        loop {
            // Every command waiting, in order.
            loop {
                match commands.try_recv() {
                    Ok(EngineCommand::Attach { key, out }) => {
                        self.views.insert(
                            key,
                            View {
                                out,
                                sent: Sent::default(),
                                styles: StyleTable::default(),
                                redraw: true,
                                exit_told: false,
                            },
                        );
                    }
                    Ok(EngineCommand::Detach { key }) => {
                        self.views.remove(&key);
                    }
                    Ok(EngineCommand::Input(bytes)) => {
                        if self.exit.is_none() {
                            pending_input.extend(bytes);
                        }
                    }
                    Ok(EngineCommand::Resize(size)) => {
                        let Ok(size) = TerminalSize::new(size.cols, size.rows) else {
                            continue;
                        };
                        let Some(next) = generation.next() else {
                            return self.fail("terminal resized too many times");
                        };
                        generation = next;
                        if self.terminal.resize(size, generation).is_err() {
                            return self.fail("terminal resize failed");
                        }
                        if self.exit.is_none() {
                            match session.resize(size) {
                                Ok(operation) => resizing = Some(operation),
                                Err(_) => return self.fail("session resize failed"),
                            }
                        }
                        dirty = true;
                    }
                    Ok(EngineCommand::History { key, start, count }) => {
                        self.history(&key, start, count);
                    }
                    Ok(EngineCommand::Close) | Err(std_mpsc::TryRecvError::Disconnected) => {
                        return;
                    }
                    Err(std_mpsc::TryRecvError::Empty) => break,
                }
            }

            // A write says how much the OS took; the rest goes back first.
            let finished = match &mut writing {
                Some((operation, _)) => poll(operation),
                None => None,
            };
            if let Some(outcome) = finished {
                if let Some((_, chunk)) = writing.take() {
                    let accepted = outcome.written.min(chunk.len());
                    for byte in chunk[accepted..].iter().rev() {
                        pending_input.push_front(*byte);
                    }
                }
                if outcome.error.is_some() && self.exit.is_none() {
                    pending_input.clear();
                }
            }
            if writing.is_none() && !pending_input.is_empty() && self.exit.is_none() {
                let take = pending_input.len().min(input_chunk);
                let bytes: Vec<u8> = pending_input.drain(..take).collect();
                if let Ok(operation) = session.write(&bytes) {
                    writing = Some((operation, bytes));
                }
            }
            if let Some(operation) = &mut resizing {
                if poll(operation).is_some() {
                    resizing = None;
                }
            }

            let mut fed = false;
            if self.exit.is_none() {
                loop {
                    match observer.try_next() {
                        Ok(Some(OutputEvent::Replay(ReplayPage::Bytes { bytes, .. }))) => {
                            for chunk in bytes.chunks(4096) {
                                match self.terminal.feed(chunk) {
                                    // A query such as DSR is answered by the
                                    // terminal; the answer is the program's input.
                                    Ok(effects) => pending_input.extend(effects.0),
                                    Err(_) => return self.fail("terminal rejected output"),
                                }
                            }
                            fed = true;
                        }
                        Ok(Some(OutputEvent::Replay(ReplayPage::Gap { .. }))) => {
                            return self.fail("the screen fell behind the program's output");
                        }
                        Ok(Some(OutputEvent::Complete(completion))) => {
                            let status = match completion.status.exit {
                                Some(ExitStatus::Code(code)) => Some(code),
                                _ => None,
                            };
                            self.exit = Some(status);
                            events.exited(&self.id, status);
                            dirty = true;
                            break;
                        }
                        Ok(Some(_)) | Ok(None) => break,
                        Err(_) => return self.fail("terminal output failed"),
                    }
                }
            }
            dirty |= fed;

            let redraw = self.views.values().any(|view| view.redraw);
            let now = clock::os::now();
            if (dirty && now >= next_frame) || redraw || (dirty && self.exit.is_some()) {
                next_frame = now + FRAME_INTERVAL;
                if !self.project(dirty) {
                    return;
                }
                dirty = false;
            }

            if !fed && writing.is_none() && pending_input.is_empty() {
                clock::os::sleep(Duration::from_millis(if dirty { 1 } else { 4 }));
            }
        }
    }

    /// Bring every view up to date: all of them when the screen changed,
    /// else only those owed a whole redraw. False when the model failed.
    fn project(&mut self, changed: bool) -> bool {
        let view = match self.terminal.view() {
            Ok(view) => view,
            Err(_) => {
                self.fail("the screen could not be read");
                return false;
            }
        };
        self.palette = view.palette.clone();
        let (total, scrollback) = match self.terminal.history(u64::MAX, 0) {
            Ok(counts) => (counts.total, counts.scrollback),
            Err(_) => (0, 0),
        };
        let exit = self.exit;
        let id = self.id.clone();
        for (key, target) in &mut self.views {
            if !changed && !target.redraw {
                continue;
            }
            let fresh = std::mem::take(&mut target.redraw);
            if fresh {
                target.sent = Sent::default();
                target.styles = StyleTable::default();
            }
            if fresh {
                target.exit_told = false;
            }
            let mut messages = split(target.sent.diff(
                &view,
                &mut target.styles,
                total,
                scrollback,
            ));
            if let Some(status) = exit {
                if !target.exit_told {
                    messages.push(encode(&ServerMessage::Exit { v: VERSION, status }));
                }
            }
            if deliver(&id, key, target, messages) {
                target.exit_told |= exit.is_some();
            } else {
                target.redraw = true;
            }
        }
        true
    }

    fn history(&mut self, key: &ViewKey, start: u64, count: u16) {
        let Some(target) = self.views.get_mut(key) else {
            return;
        };
        let history = match self.terminal.history(start, count.min(500)) {
            Ok(history) => history,
            Err(_) => return,
        };
        let (mut rows, pending, reset) =
            wire::encode_history(&history, &mut target.styles, &self.palette);
        let mut messages = Vec::new();
        if reset {
            // The view must drop its styles before new ids arrive; its next
            // frame redraws it whole against the new table.
            messages.push(encode(&ServerMessage::Ready {
                v: VERSION,
                cols: history.cols,
                rows: u16::try_from(history.total.min(u64::from(u16::MAX)))
                    .unwrap_or(0)
                    .max(1),
                palette: wire::WirePalette::from(&self.palette),
            }));
            target.sent = Sent::default();
            target.redraw = true;
        }
        if !pending.is_empty() {
            messages.push(encode(&ServerMessage::Styles {
                v: VERSION,
                styles: pending,
            }));
        }
        // Keep the reply under the budget: drop rows from the end, and the
        // view asks again from where this one stopped.
        loop {
            let message = encode(&ServerMessage::History {
                v: VERSION,
                start: history.start,
                rows: rows.clone(),
                total: history.total,
                scrollback: history.scrollback,
            });
            if message.len() <= MESSAGE_BUDGET || rows.len() <= 1 {
                messages.push(message);
                break;
            }
            let keep = (rows.len() * MESSAGE_BUDGET / message.len()).clamp(1, rows.len() - 1);
            rows.truncate(keep);
        }
        let id = self.id.clone();
        if !deliver(&id, key, target, messages) {
            target.redraw = true;
        }
    }

    /// Tell every view the terminal can no longer be drawn.
    fn fail(&mut self, message: &str) {
        let text = encode(&ServerMessage::Error {
            v: VERSION,
            message: message.to_owned(),
        });
        let id = self.id.clone();
        for (key, target) in &mut self.views {
            let _ = deliver(&id, key, target, vec![text.clone()]);
        }
        tracing::warn!(terminal = %self.id, reason = message, "terminal ended");
    }
}

fn encode(message: &ServerMessage) -> String {
    serde_json::to_string(message).unwrap_or_else(|_| "{}".to_owned())
}

/// Encode `messages`, cutting any frame over the budget into several frames
/// of whole rows, each with the same cursor and counts.
fn split(messages: Vec<ServerMessage>) -> Vec<String> {
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        let text = encode(&message);
        if text.len() <= MESSAGE_BUDGET {
            out.push(text);
            continue;
        }
        let ServerMessage::Frame {
            v,
            seq,
            cursor,
            rows,
            total,
            scrollback,
        } = message
        else {
            out.push(text);
            continue;
        };
        let mut part = Vec::new();
        let mut part_bytes = 0usize;
        for row in rows {
            let row_bytes = serde_json::to_string(&row).map(|s| s.len()).unwrap_or(0) + 1;
            if !part.is_empty() && part_bytes + row_bytes > MESSAGE_BUDGET - 256 {
                out.push(encode(&ServerMessage::Frame {
                    v,
                    seq,
                    cursor: cursor.clone(),
                    rows: std::mem::take(&mut part),
                    total,
                    scrollback,
                }));
                part_bytes = 0;
            }
            part_bytes += row_bytes;
            part.push(row);
        }
        out.push(encode(&ServerMessage::Frame {
            v,
            seq,
            cursor,
            rows: part,
            total,
            scrollback,
        }));
    }
    out
}

/// Queue `messages` for one view. False when its connection's queue was
/// full or gone: the view is then redrawn whole when there is room.
fn deliver(id: &str, key: &ViewKey, target: &mut View, messages: Vec<String>) -> bool {
    if target.out.capacity() < messages.len() {
        return false;
    }
    for message in messages {
        let signal = TerminalSignal::Output {
            terminal_id: id.to_owned(),
            view_id: key.view.clone(),
            message,
        };
        if target.out.try_send(signal).is_err() {
            return false;
        }
    }
    true
}
