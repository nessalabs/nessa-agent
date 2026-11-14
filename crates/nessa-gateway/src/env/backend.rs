use super::{EnvironmentError, Stage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UptimeBackend {
    Monotonic,
    Fixed(u64),
}

impl UptimeBackend {
    pub fn parse(
        stage: Stage,
        backend: Option<&str>,
        fixed: Option<&str>,
    ) -> Result<Self, EnvironmentError> {
        match (backend.unwrap_or("monotonic"), fixed) {
            ("monotonic", None) => Ok(Self::Monotonic),
            ("fixed", Some(value)) if matches!(stage, Stage::Dev | Stage::Ci) => {
                if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(EnvironmentError::Backend(
                        "NESSA_UPTIME_FIXED_MS must be an unsigned integer",
                    ));
                }
                value
                    .parse()
                    .map(Self::Fixed)
                    .map_err(|_| EnvironmentError::Backend("NESSA_UPTIME_FIXED_MS exceeds u64"))
            }
            ("fixed", _) if !matches!(stage, Stage::Dev | Stage::Ci) => Err(
                EnvironmentError::Backend("fixed uptime is only allowed in dev/ci"),
            ),
            ("fixed", None) => Err(EnvironmentError::Backend(
                "fixed uptime requires NESSA_UPTIME_FIXED_MS",
            )),
            ("monotonic", Some(_)) => Err(EnvironmentError::Backend(
                "NESSA_UPTIME_FIXED_MS requires fixed uptime",
            )),
            _ => Err(EnvironmentError::Backend(
                "NESSA_UPTIME_BACKEND must be monotonic or fixed",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::env::{Environment, MockEnv};

    #[test]
    fn invalid_or_non_dev_selections_fail() {
        for (stage, backend, value) in [
            ("prod", "fixed", "1"),
            ("alpha", "fixed", "1"),
            ("dev", "unknown", "1"),
            ("dev", "monotonic", "1"),
            ("ci", "fixed", "-1"),
            ("ci", "fixed", ""),
            ("dev", "fixed", "18446744073709551616"),
        ] {
            assert!(Environment::load(
                &MockEnv::new()
                    .set("NESSA_STAGE", stage)
                    .set("NESSA_UPTIME_BACKEND", backend)
                    .set("NESSA_UPTIME_FIXED_MS", value)
            )
            .is_err());
        }
        assert!(Environment::load(&MockEnv::new().set("NESSA_UPTIME_BACKEND", "fixed")).is_err());
    }
}
