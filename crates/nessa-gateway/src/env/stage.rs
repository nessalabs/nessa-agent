use std::fmt;

/// Where this server process is running.
/// Drives auth, logging, and other policy — not scattered one-off flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Local development on your machine.
    Dev,
    /// Shared testing environment (staging, dogfood).
    Alpha,
    /// Automated CI/CD runs.
    Ci,
    /// Production.
    Prod,
}

impl Stage {
    pub fn parse(value: &str) -> Result<Self, InvalidStage> {
        match value {
            "dev" => Ok(Self::Dev),
            "alpha" => Ok(Self::Alpha),
            "ci" => Ok(Self::Ci),
            "prod" => Ok(Self::Prod),
            _ => Err(InvalidStage(value.to_owned())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Alpha => "alpha",
            Self::Ci => "ci",
            Self::Prod => "prod",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidStage(pub String);

impl fmt::Display for InvalidStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid stage {:?}; expected dev, alpha, ci, or prod",
            self.0
        )
    }
}

impl std::error::Error for InvalidStage {}
