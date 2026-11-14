use super::error::ReadError;
use std::collections::HashMap;
use std::env::VarError;

/// Reads a single configuration value by env key.
pub trait EnvSource {
    fn get(&self, key: &str) -> Result<Option<String>, ReadError>;
}

/// Production source: reads from the process environment.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemEnv;

impl EnvSource for SystemEnv {
    fn get(&self, key: &str) -> Result<Option<String>, ReadError> {
        match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(VarError::NotPresent) => Ok(None),
            Err(VarError::NotUnicode(_)) => Err(ReadError::NotUnicode {
                key: key.to_string(),
            }),
        }
    }
}

/// Test source: in-memory map, safe for parallel tests.
#[derive(Debug, Default, Clone)]
pub struct MockEnv {
    values: HashMap<String, String>,
}

impl MockEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.values.insert(key.into(), value.into());
        self
    }
}

impl EnvSource for MockEnv {
    fn get(&self, key: &str) -> Result<Option<String>, ReadError> {
        Ok(self.values.get(key).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::PORT;

    #[test]
    fn mock_env_isolated_between_instances() {
        let a = MockEnv::new().set(PORT, "1111");
        let b = MockEnv::new().set(PORT, "2222");

        assert_eq!(a.get(PORT).unwrap(), Some("1111".to_string()));
        assert_eq!(b.get(PORT).unwrap(), Some("2222".to_string()));
    }
}
