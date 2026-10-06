//! The clock and the entropy the owner asks for. Both read the process
//! boundary here, not in the chart.
use crate::mcp_authorization::application::{AuthClock, Entropy};

pub struct SystemAuthClock;

impl AuthClock for SystemAuthClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }
}

pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn bytes(&self, len: usize) -> Result<Vec<u8>, ()> {
        let mut bytes = vec![0; len];
        getrandom::fill(&mut bytes).map_err(|_| ())?;
        Ok(bytes)
    }
}
