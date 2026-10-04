//! The monotonic clock port.

/// Monotonic elapsed runtime. Callers own this port; adapters own clocks.
///
/// Distinct from Auth's wall-clock port: this one only ever measures how long
/// something has taken, never what time it is.
pub trait Clock: Send + Sync {
    /// Milliseconds since an arbitrary fixed start, never decreasing.
    fn elapsed_ms(&self) -> u64;
}
