//! One desktop host's retained lease, shared with admitted lifecycle workers.
//!
//! The gateway host port supplies physical acquisition. This owner serializes
//! acquisition and retains its result across attempts; a caller only borrows it.
use std::sync::{Arc, Mutex};

/// A physical host lease whose resource is released after its final owner.
#[derive(Clone)]
pub struct DesktopHostLease {
    _resource: Arc<dyn Send + Sync>,
}

impl DesktopHostLease {
    pub fn new(resource: impl Send + Sync + 'static) -> Self {
        Self {
            _resource: Arc::new(resource),
        }
    }
}

/// A nonblocking acquisition observation; busy is not an IO failure.
pub enum DesktopHostClaim {
    Acquired(DesktopHostLease),
    #[cfg_attr(
        not(any(target_os = "macos", test)),
        allow(
            dead_code,
            reason = "only the macOS native adapter arbitrates desktop owners"
        )
    )]
    Busy,
}

#[derive(Default)]
pub(super) struct DesktopHostOwnership {
    lease: Mutex<Option<DesktopHostLease>>,
}

impl DesktopHostOwnership {
    pub(super) fn retained(&self) -> Option<DesktopHostLease> {
        self.lease
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// H3–H6/H11/H12 in `docs/design/desktop-host-ownership.md`.
    /// `wait` owns the injected monotonic acquisition deadline.
    pub(super) fn claim<E>(
        &self,
        mut acquire: impl FnMut() -> Result<DesktopHostClaim, E>,
        mut wait: impl FnMut() -> bool,
    ) -> Result<DesktopHostClaim, E> {
        let mut held = self.lease.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(lease) = held.as_ref() {
            return Ok(DesktopHostClaim::Acquired(lease.clone()));
        }
        loop {
            match acquire()? {
                DesktopHostClaim::Acquired(lease) => {
                    *held = Some(lease.clone());
                    return Ok(DesktopHostClaim::Acquired(lease));
                }
                DesktopHostClaim::Busy if wait() => {}
                DesktopHostClaim::Busy => return Ok(DesktopHostClaim::Busy),
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/gateway/application/host_ownership_owner.rs"]
mod tests;
