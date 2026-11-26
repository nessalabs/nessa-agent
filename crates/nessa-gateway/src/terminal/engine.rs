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
        let command = build_command_spec(spec)?;
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
        // The terminal's first process, under which every program that may
        // report runs. Without it no report can be checked, so none is taken.
        let resume = match (&spec.resume_token, session.process_id()) {
            (Some(token), Ok(root)) => Some(super::ResumeTracker::new(
                token.clone(),
                root,
                spec.resume.is_some(),
            )),
            _ => None,
        };
        let processes = events.processes();
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
                    program_status: super::ProgramStatusTracker::new(),
                    resume,
                    processes,
                };
                let mut transport = LocalPtyTransport {
                    runtime,
                    session_id,
                    session,
                    observer,
                    writing: None,
                    resizing: None,
                    input_chunk,
                };
                run.drive(&mut transport, commands, &*events);
                transport.finish();
            })
            .map_err(|error| TerminalError::Spawn(error.to_string()))?;
        Ok(Box::new(Thread(Mutex::new(sender))))
    }
}

fn build_command_spec(spec: &StartSpec) -> Result<CommandSpec, TerminalError> {
    let mut environment: Vec<(OsString, OsString)> = vec![
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "Nessa".into()),
    ];
    // A login shell reads the person's profile, so PATH and the rest are
    // theirs, not the gateway's. TERM says which sequences it may send.
    let mut arguments: Vec<OsString> = vec!["-l".into()];
    let mut removed: Vec<OsString> = vec!["NO_COLOR".into(), "FORCE_COLOR".into()];
    // OSC 7577: the first process alone is given this terminal's own token,
    // and with resume off nothing is inherited, not even the gateway's own.
    match &spec.resume_token {
        Some(token) => environment.push((super::TOKEN_VAR.into(), token.into())),
        None => removed.push(super::TOKEN_VAR.into()),
    }
    let mut folder = spec.folder.clone();
    if let Some(resume) = &spec.resume {
        // A program a restart brings back: the shell's startup files run,
        // then `env` starts it in its own folder.
        let resumed = resume
            .shell_arguments(&spec.shell)
            .ok_or(TerminalError::Invalid)?;
        arguments = resumed.into_iter().map(OsString::from).collect();
        folder = resume.cwd.clone();
    } else if let TerminalKind::Run { command } = &spec.kind {
        // Drawn as it would be typed: the person's own prompt, the
        // command, then what it prints. The text goes in the environment,
        // never into the script, so no quoting can change what runs; eval
        // keeps its exit status, and the profile's aliases.
        arguments.extend(["-i".into(), "-c".into(), run_script(&spec.shell).into()]);
        environment.push(("NESSA_RUN_COMMAND".into(), command.into()));
    }
    CommandSpec::new(spec.shell.clone(), folder, arguments)
        .and_then(|command| {
            command.with_environment(EnvironmentPolicy::Inherit, removed, environment)
        })
        .map_err(|error| TerminalError::Spawn(format!("{error:?}")))
}

enum TransportEvent {
    Bytes(Vec<u8>),
    Gap,
    Exited(Option<i32>),
}

struct LocalPtyTransport {
    runtime: Arc<Runtime>,
    session_id: SessionId,
    session: pty_runtime::Session,
    observer: pty_runtime::Attachment,
    writing: Option<(ProcessOperation<WriteOutcome>, Vec<u8>)>,
    resizing: Option<ProcessOperation<Result<(), pty_runtime::ProcessError>>>,
    input_chunk: usize,
}

impl LocalPtyTransport {
    fn finish(self) {
        let _ = self.session.cancel();
        if let Ok(mut completion) = self.session.wait() {
            let deadline = clock::os::now() + CLOSE_GRACE;
            while clock::os::now() < deadline && poll(&mut completion).is_none() {
                clock::os::sleep(Duration::from_millis(5));
            }
        }
        let _ = self.runtime.forget(&self.session_id);
    }

    fn pump_io(&mut self, pending_input: &mut VecDeque<u8>, exited: bool) {
        // A write says how much the OS took; the rest goes back first.
        let finished = match &mut self.writing {
            Some((operation, _)) => poll(operation),
            None => None,
        };
        if let Some(outcome) = finished {
            if let Some((_, chunk)) = self.writing.take() {
                let accepted = outcome.written.min(chunk.len());
                for byte in chunk[accepted..].iter().rev() {
                    pending_input.push_front(*byte);
                }
            }
            if outcome.error.is_some() && !exited {
                pending_input.clear();
            }
        }
        if self.writing.is_none() && !pending_input.is_empty() && !exited {
            let take = pending_input.len().min(self.input_chunk);
            let bytes: Vec<u8> = pending_input.drain(..take).collect();
            if let Ok(operation) = self.session.write(&bytes) {
                self.writing = Some((operation, bytes));
            }
        }
        if let Some(operation) = &mut self.resizing {
            if poll(operation).is_some() {
                self.resizing = None;
            }
        }
    }

    fn has_pending_write(&self) -> bool {
        self.writing.is_some()
    }

    fn resize(&mut self, size: TerminalSize) -> Result<(), &'static str> {
        match self.session.resize(size) {
            Ok(operation) => {
                self.resizing = Some(operation);
                Ok(())
            }
            Err(_) => Err("session resize failed"),
        }
    }

    fn try_next(&mut self) -> Result<Option<TransportEvent>, &'static str> {
        match self.observer.try_next() {
            Ok(Some(OutputEvent::Replay(ReplayPage::Bytes { bytes, .. }))) => {
                Ok(Some(TransportEvent::Bytes(bytes)))
            }
            Ok(Some(OutputEvent::Replay(ReplayPage::Gap { .. }))) => Ok(Some(TransportEvent::Gap)),
            Ok(Some(OutputEvent::Complete(completion))) => {
                let status = match completion.status.exit {
                    Some(ExitStatus::Code(code)) => Some(code),
                    _ => None,
                };
                Ok(Some(TransportEvent::Exited(status)))
            }
            Ok(Some(_)) | Ok(None) => Ok(None),
            Err(_) => Err("terminal output failed"),
        }
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
    program_status: super::ProgramStatusTracker,
    /// The terminal's OSC 7577 side, when it was given a token.
    resume: Option<super::ResumeTracker>,
    processes: Arc<dyn super::Processes>,
}

impl Run {
    fn drive(
        &mut self,
        transport: &mut LocalPtyTransport,
        commands: std_mpsc::Receiver<EngineCommand>,
        events: &dyn EngineEvents,
    ) {
        let mut pending_input: VecDeque<u8> = VecDeque::new();
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
                            return self.fail("terminal resized too many times", events);
                        };
                        generation = next;
                        if self.terminal.resize(size, generation).is_err() {
                            return self.fail("terminal resize failed", events);
                        }
                        if self.exit.is_none() {
                            if let Err(reason) = transport.resize(size) {
                                return self.fail(reason, events);
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

            transport.pump_io(&mut pending_input, self.exit.is_some());

            let mut fed = false;
            if self.exit.is_none() {
                loop {
                    match transport.try_next() {
                        Ok(Some(TransportEvent::Bytes(bytes))) => {
                            let mut status_changed = false;
                            for chunk in bytes.chunks(4096) {
                                // Scan OSC 7501 before feeding Ghostty so any `?` probe reply is
                                // queued ahead of a trailing DA1 (`CSI c`) reply in the same write.
                                let scan = self.program_status.feed(chunk);
                                if !scan.replies.is_empty() {
                                    pending_input.extend(scan.replies);
                                }
                                status_changed |= scan.changed;
                                self.take_resume_reports(scan.resume_reports, events);
                                match self.terminal.feed(chunk) {
                                    // A query such as DSR is answered by the
                                    // terminal; the answer is the program's input.
                                    Ok(effects) => pending_input.extend(effects.0),
                                    Err(_) => return self.fail("terminal rejected output", events),
                                }
                            }
                            if status_changed {
                                events.program_status(
                                    &self.id,
                                    self.program_status.current().cloned(),
                                );
                            }
                            fed = true;
                        }
                        Ok(Some(TransportEvent::Gap)) => {
                            return self
                                .fail("the screen fell behind the program's output", events);
                        }
                        Ok(Some(TransportEvent::Exited(status))) => {
                            self.program_status.on_exit();
                            self.exit = Some(status);
                            events.exited(&self.id, status, self.program_status.current().cloned());
                            dirty = true;
                            break;
                        }
                        Ok(None) => break,
                        Err(reason) => return self.fail(reason, events),
                    }
                }
            }
            dirty |= fed;
            if self.exit.is_none() {
                self.poll_resume_owner(events);
            }

            let redraw = self.views.values().any(|view| view.redraw);
            let now = clock::os::now();
            if (dirty && now >= next_frame) || redraw || (dirty && self.exit.is_some()) {
                next_frame = now + FRAME_INTERVAL;
                if !self.project(dirty, events) {
                    return;
                }
                dirty = false;
            }

            if !fed && !transport.has_pending_write() && pending_input.is_empty() {
                clock::os::sleep(Duration::from_millis(if dirty { 1 } else { 4 }));
            }
        }
    }

    /// Bring every view up to date: all of them when the screen changed,
    /// else only those owed a whole redraw. False when the model failed.
    fn project(&mut self, changed: bool, events: &dyn EngineEvents) -> bool {
        let view = match self.terminal.view() {
            Ok(view) => view,
            Err(_) => {
                self.fail("the screen could not be read", events);
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

    /// Check the OSC 7577 reports one read held, in order.
    fn take_resume_reports(&mut self, reports: Vec<String>, events: &dyn EngineEvents) {
        let Some(tracker) = self.resume.as_mut() else {
            return;
        };
        for body in reports {
            if let Some(change) = tracker.report(&body, wall_ms(), &*self.processes) {
                events.resume(&self.id, change);
            }
        }
    }

    /// Whether the record's owner is still there; its record goes once it
    /// has been gone a while.
    fn poll_resume_owner(&mut self, events: &dyn EngineEvents) {
        let Some(tracker) = self.resume.as_mut() else {
            return;
        };
        if let Some(change) = tracker.poll(wall_ms(), &*self.processes) {
            events.resume(&self.id, change);
        }
    }

    /// Drop live (working/blocked) program status before exiting the thread on error.
    fn clear_live_status(&mut self, events: &dyn EngineEvents) {
        if self.program_status.on_exit() {
            events.program_status(&self.id, self.program_status.current().cloned());
        }
    }

    /// Tell every view the terminal can no longer be drawn, and drop any live
    /// (working/blocked) program status before exiting the thread.
    fn fail(&mut self, message: &str, events: &dyn EngineEvents) {
        self.clear_live_status(events);
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

/// Milliseconds since the epoch: OSC 7577 times are wall-clock seconds.
fn wall_ms() -> u64 {
    clock::wall_now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
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

/// The script a Run's shell runs: its prompt as the person sees it, the
/// command after it, then the command. zsh and bash expand their own prompt;
/// another shell shows `$ `.
fn run_script(shell: &std::path::Path) -> &'static str {
    match shell.file_name().and_then(|name| name.to_str()) {
        Some("zsh") => {
            r#"print -rn -- "${(%)PROMPT}"; print -r -- "$NESSA_RUN_COMMAND"; eval "$NESSA_RUN_COMMAND""#
        }
        Some("bash") => {
            r#"if [ "${BASH_VERSINFO[0]}${BASH_VERSINFO[1]}" -ge 44 ] 2>/dev/null; then printf '%s' "${PS1@P}"; else printf '$ '; fi; printf '%s\n' "$NESSA_RUN_COMMAND"; eval "$NESSA_RUN_COMMAND""#
        }
        _ => r#"printf '$ %s\n' "$NESSA_RUN_COMMAND"; eval "$NESSA_RUN_COMMAND""#,
    }
}
