/// Monotonic elapsed runtime. The application owns this port; adapters own clocks.
pub trait Clock: Send + Sync {
    fn elapsed_ms(&self) -> u64;
}
