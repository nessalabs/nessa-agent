//! What a terminal is to the hub: its owner, size, kind, list entry, end,
//! errors, and the signals and commands that move between the hub and views.
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Terminals one person may hold at once, running or ended.
pub const MAX_TERMINALS_PER_OWNER: usize = 32;
/// Views a terminal may have at once, across every connection.
pub const MAX_VIEWS_PER_TERMINAL: usize = 16;
/// Messages waiting for one connection. A full queue drops nothing that
/// matters: the view it was for is redrawn whole on the next frame.
pub const CONNECTION_QUEUE: usize = 512;

/// Who a terminal belongs to: the organization and person that opened it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerminalOwner {
    pub organization: String,
    pub principal: String,
}

/// A grid size, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
}

impl GridSize {
    /// The size, when the wire's numbers name one pty-runtime accepts.
    pub fn new(cols: u64, rows: u64) -> Option<Self> {
        let cols = u16::try_from(cols)
            .ok()
            .filter(|c| (2..=1000).contains(c))?;
        let rows = u16::try_from(rows)
            .ok()
            .filter(|r| (1..=1000).contains(r))?;
        (u32::from(cols) * u32::from(rows) <= 1_048_576).then_some(Self { cols, rows })
    }
}

/// What a terminal was started for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalKind {
    /// A shell, from ⌘J.
    Shell,
    /// `command`, from Run in a reply, given to the login shell with `-c`.
    Run { command: String },
}

/// What to start, once the place is resolved.
#[derive(Clone, Debug)]
pub struct StartSpec {
    pub folder: PathBuf,
    pub shell: PathBuf,
    pub kind: TerminalKind,
    pub size: GridSize,
    /// The OSC 7577 token the first process is given; `None` when resume
    /// is off, and then the variable is taken out of what it inherits.
    pub resume_token: Option<String>,
    /// A program a restart brings back: the shell starts, then runs it.
    pub resume: Option<ResumeStart>,
}

/// A saved command a restart runs again in its terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeStart {
    /// The program, then its arguments, each passed as one argument.
    pub argv: Vec<String>,
    /// Where it runs.
    pub cwd: PathBuf,
}

impl ResumeStart {
    /// The arguments that start `shell` to run this command: the shell's
    /// startup files run first, as for any terminal, then `env` runs the
    /// program, so a function, alias or builtin of the same name never runs
    /// in its place, and each element is one argument, never shell text.
    /// When the program ends, a fresh login shell takes its place.
    /// `None` for a shell this cannot be said to, or a program `env` would
    /// read as a setting (`NAME=value`).
    pub fn shell_arguments(&self, shell: &std::path::Path) -> Option<Vec<String>> {
        let program = self.argv.first()?;
        // pty-runtime takes at most 128 arguments; six are the script's.
        if self.argv.len() > 122 {
            return None;
        }
        if program.is_empty() || program.contains('=') || program.starts_with('-') {
            return None;
        }
        let name = shell.file_name()?.to_str()?;
        if !RESUMABLE_SHELLS.contains(&name) {
            return None;
        }
        let script = r#"shell=$1; shift; printf '\033[2m%s\033[0m\n' 'Resumed after Nessa restarted'; /usr/bin/env -- "$@"; exec "$shell" -l"#;
        let mut arguments = vec![
            "-l".to_owned(),
            "-i".to_owned(),
            "-c".to_owned(),
            script.to_owned(),
            "nessa-resume".to_owned(),
            shell.to_string_lossy().into_owned(),
        ];
        arguments.extend(self.argv.iter().cloned());
        Some(arguments)
    }
}

/// Shells whose `-c` takes the script's arguments as `$1…`: the ones a
/// resumed program can be started from.
const RESUMABLE_SHELLS: [&str; 10] = [
    "sh", "bash", "zsh", "dash", "ksh", "ksh93", "mksh", "oksh", "yash", "ash",
];

/// A terminal a restart opens again, by its old id and name.
#[derive(Clone, Debug)]
pub struct Reopened {
    pub id: String,
    pub title: String,
    pub start: ResumeStart,
    /// The kept record, as the terminal holds it from its first moment.
    pub saved: crate::terminal::SavedRecord,
}

/// One terminal as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalEntry {
    pub id: String,
    pub request_id: String,
    pub title: String,
    pub kind: TerminalKind,
    pub channel_id: Option<String>,
    pub conversation_id: Option<String>,
    pub folder: String,
    pub shell: String,
    pub runs_on: String,
    pub started_at_ms: u64,
    pub ended: Option<TerminalEnd>,
    pub program_status: Option<StampedProgramStatus>,
    /// The command the terminal's program asked to come back as, while
    /// that record is held, shown escaped.
    pub resume_command: Option<String>,
    /// When a restart started this terminal again.
    pub resumed_at_ms: Option<u64>,
}

/// Active OSC 7501 program status paired with when it last changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StampedProgramStatus {
    pub status: super::ProgramStatus,
    pub updated_at_ms: u64,
}

/// How a terminal's program ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalEnd {
    /// The exit code; `None` when a signal ended it.
    pub status: Option<i32>,
    pub at_ms: u64,
}

/// Why the hub refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalError {
    /// No terminal engine in this build, or it could not start.
    Unavailable,
    /// No such terminal of the caller's.
    NotFound,
    /// The caller holds as many as they may, or the terminal as many views.
    Limit,
    /// The request named nothing that can be run.
    Invalid,
    /// The program could not be started.
    Spawn(String),
    /// A kept program's file could not be changed.
    Storage(String),
}

/// A message for one view, as pty-runtime's client protocol JSON text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalSignal {
    Output {
        terminal_id: String,
        view_id: String,
        message: String,
    },
    /// The owner's list changed.
    Listed,
}

/// One view of a terminal: the connection it is on and the window's id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ViewKey {
    pub connection: u64,
    pub view: String,
}

/// What the engine thread of one terminal is asked to do.
pub enum EngineCommand {
    /// Start showing this view: a checkpoint, then live frames, on `out`.
    Attach {
        key: ViewKey,
        out: mpsc::Sender<TerminalSignal>,
    },
    Detach {
        key: ViewKey,
    },
    Input(Vec<u8>),
    Resize(GridSize),
    History {
        key: ViewKey,
        start: u64,
        count: u16,
    },
    /// Hang up the program, kill it after 5 s, and end the thread.
    Close,
}
