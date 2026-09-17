use std::{error::Error, fmt, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandValidationError {
    Empty,
    ContainsNul,
    TooLong { max_bytes: usize },
    TimeoutOutOfRange { seconds: u64 },
}

impl fmt::Display for CommandValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("command must be nonempty"),
            Self::ContainsNul => f.write_str("command must not contain NUL"),
            Self::TooLong { max_bytes } => write!(f, "command exceeds {max_bytes} bytes"),
            Self::TimeoutOutOfRange { seconds } => {
                write!(f, "timeout {seconds} seconds is outside 1..=3600")
            }
        }
    }
}
impl Error for CommandValidationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    text: String,
    timeout: Duration,
}
impl ShellCommand {
    pub fn new(text: String, timeout_seconds: u64) -> Result<Self, CommandValidationError> {
        if text.trim().is_empty() {
            return Err(CommandValidationError::Empty);
        }
        if text.contains('\0') {
            return Err(CommandValidationError::ContainsNul);
        }
        if text.len() > 32_768 {
            return Err(CommandValidationError::TooLong { max_bytes: 32_768 });
        }
        if !(1..=3600).contains(&timeout_seconds) {
            return Err(CommandValidationError::TimeoutOutOfRange {
                seconds: timeout_seconds,
            });
        }
        Ok(Self {
            text,
            timeout: Duration::from_secs(timeout_seconds),
        })
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[cfg(test)]
#[path = "../../../tests/shell/domain/command.rs"]
mod tests;
