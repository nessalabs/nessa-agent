/// Monotonic elapsed runtime. The application owns this port; adapters own clocks.
pub trait UptimeClock: Send + Sync {
    fn elapsed_ms(&self) -> u64;
}
