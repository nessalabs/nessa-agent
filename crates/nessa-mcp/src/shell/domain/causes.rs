#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopCause {
    ClientClosed,
    ClientCancelled,
    HostShutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunCause {
    Exited,
    Timeout,
    Stopped(StopCause),
    SpawnFailed,
    WaitFailed,
    AuditFailed,
}
