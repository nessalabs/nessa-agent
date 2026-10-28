use nessa_protocol::product_contract::generated::{
    BACKGROUND_TASK_ID_PATTERN, MAX_BACKGROUND_TASK_ID_CHARACTERS,
    MIN_BACKGROUND_TASK_ID_CHARACTERS,
};
use regex::Regex;
use std::sync::OnceLock;

/// Identity chosen once for a task in its originating conversation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BackgroundTaskId(Box<str>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskInputError {
    Identity,
    Command,
    Timeout,
}

impl BackgroundTaskId {
    pub const MAX_CHARACTERS: usize = MAX_BACKGROUND_TASK_ID_CHARACTERS;
    pub const MIN_CHARACTERS: usize = MIN_BACKGROUND_TASK_ID_CHARACTERS;
    pub const PATTERN: &'static str = BACKGROUND_TASK_ID_PATTERN;

    pub fn new(value: impl Into<String>) -> Result<Self, TaskInputError> {
        let value = value.into();
        static PATTERN: OnceLock<Regex> = OnceLock::new();
        if value.chars().count() < Self::MIN_CHARACTERS
            || value.chars().count() > Self::MAX_CHARACTERS
            || !PATTERN
                .get_or_init(|| {
                    Regex::new(Self::PATTERN).expect("generated background identity pattern")
                })
                .is_match(&value)
        {
            return Err(TaskInputError::Identity);
        }
        Ok(Self(value.into_boxed_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One shell command, with an explicit execution deadline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundCommand {
    text: Box<str>,
    timeout_seconds: u64,
}

impl BackgroundCommand {
    // A JSON-escaped command and its task/timeout fields must fit the SDK's
    // retained native call input, which admission uses for row correlation.
    pub const MAX_BYTES: usize =
        (nessa_sdk::domain::agent_execution::tools::MAX_TOOL_INPUT_BYTES - 256) / 6;
    pub const MAX_TIMEOUT_SECONDS: u64 = 3600;
    pub const DEFAULT_TIMEOUT_SECONDS: u64 = 120;

    pub fn new(text: impl Into<String>, timeout_seconds: u64) -> Result<Self, TaskInputError> {
        let text = text.into();
        if text.trim().is_empty() || text.len() > Self::MAX_BYTES || text.contains('\0') {
            return Err(TaskInputError::Command);
        }
        if !(1..=Self::MAX_TIMEOUT_SECONDS).contains(&timeout_seconds) {
            return Err(TaskInputError::Timeout);
        }
        Ok(Self {
            text: text.into_boxed_str(),
            timeout_seconds,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskCause {
    Exited,
    Cancelled,
    TimedOut,
    SpawnFailed,
    WorkerFailed,
    Interrupted,
}

/// Actual process evidence, separate from why the application stopped waiting.
/// A nonzero exit is a command failure; missing cleanup is not confirmed release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskOutcome {
    pub cause: TaskCause,
    pub scope: Option<u64>,
    pub process: Option<u64>,
    pub os_pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub cleanup_verified: bool,
    pub stdout: String,
    pub stderr: String,
    pub omitted_bytes: u64,
    pub output_complete: bool,
    pub error: Option<String>,
}

impl TaskOutcome {
    /// Kept output across both streams, before UTF-8 decoding.
    pub const MAX_OUTPUT_BYTES: usize = 16 * 1024;

    pub(super) fn is_consistent(&self) -> bool {
        if self.cause == TaskCause::Interrupted {
            return self.scope.is_none()
                && self.process.is_none()
                && self.os_pid.is_none()
                && self.exit_code.is_none()
                && self.signal.is_none()
                && !self.cleanup_verified
                && !self.output_complete;
        }
        (self.process.is_none() || self.scope.is_some())
            && (self.os_pid.is_none() || self.process.is_some())
            && (self.cause != TaskCause::Exited || self.process.is_some())
    }

    pub fn succeeded(&self) -> bool {
        self.cause == TaskCause::Exited
            && self.exit_code == Some(0)
            && self.cleanup_verified
            && self.output_complete
            && self.error.is_none()
    }

    pub fn interrupted() -> Self {
        Self {
            cause: TaskCause::Interrupted, scope: None, process: None, os_pid: None,
            exit_code: None, signal: None, cleanup_verified: false,
            stdout: String::new(), stderr: String::new(), omitted_bytes: 0,
            output_complete: false,
            error: Some("The gateway stopped before this task recorded its outcome. Its command was not restarted and process cleanup is unconfirmed.".into()),
        }
    }
}
