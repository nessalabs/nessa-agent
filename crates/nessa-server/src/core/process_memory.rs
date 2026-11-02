//! Linux process-local dump and inspection defenses before any command runs.
//!
//! `bootstrap -> before -> ProcessMemory <- NativeProcessMemory` names calls and
//! the injected OS boundary. Unit substitutes verify refusal before dispatch;
//! native checks run only in a disposable child process. This is not filesystem
//! isolation, locked memory, or protection from root/compromised process code.
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MemoryState {
    pub(super) core_soft: u64,
    pub(super) core_hard: u64,
    pub(super) dumpable: i32,
}
impl MemoryState {
    fn protected(self) -> bool {
        self.core_soft == 0 && self.core_hard == 0 && self.dumpable == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    ReadCoreLimit,
    ReadDumpability,
    SetCoreLimit,
    SetDumpability,
    Verification,
}
impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReadCoreLimit => "could not read process core-dump limits",
            Self::ReadDumpability => "could not read process dumpability",
            Self::SetCoreLimit => "could not disable process core dumps",
            Self::SetDumpability => "could not make this process nondumpable",
            Self::Verification => "process memory protections were not confirmed",
        })
    }
}

pub(super) trait ProcessMemory {
    fn state(&self) -> Result<MemoryState, Failure>;
    fn disable_core_dumps(&self) -> Result<(), Failure>;
    fn disable_dumpability(&self) -> Result<(), Failure>;
}

/// Dispatch only after the effective native state confirms both protections.
/// The `refusals_do_not_dispatch` and `contradictory_receipts_do_not_dispatch`
/// tests enforce this ordering independently of the native adapter.
pub(super) fn before<T>(
    process: &impl ProcessMemory,
    dispatch: impl FnOnce() -> T,
) -> Result<T, Failure> {
    let state = process.state()?;
    if state.core_soft != 0 || state.core_hard != 0 {
        process.disable_core_dumps()?;
    }
    if state.dumpable != 0 {
        process.disable_dumpability()?;
    }
    if !process.state()?.protected() {
        return Err(Failure::Verification);
    }
    Ok(dispatch())
}

pub(super) struct NativeProcessMemory;
impl ProcessMemory for NativeProcessMemory {
    fn state(&self) -> Result<MemoryState, Failure> {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `limit` is a valid writable rlimit for this process.
        if unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut limit) } != 0 {
            return Err(Failure::ReadCoreLimit);
        }
        // SAFETY: PR_GET_DUMPABLE takes no additional arguments and reads only
        // this process's dumpability. No other process is inspected.
        let dumpable =
            unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0_usize, 0_usize, 0_usize, 0_usize) };
        if dumpable < 0 {
            return Err(Failure::ReadDumpability);
        }
        Ok(MemoryState {
            core_soft: limit.rlim_cur,
            core_hard: limit.rlim_max,
            dumpable,
        })
    }
    fn disable_core_dumps(&self) -> Result<(), Failure> {
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `limit` is valid and affects only this process and its future
        // children, not a system-wide or another process's limit.
        if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } != 0 {
            return Err(Failure::SetCoreLimit);
        }
        Ok(())
    }
    fn disable_dumpability(&self) -> Result<(), Failure> {
        // SAFETY: PR_SET_DUMPABLE accepts integer zero and changes only the
        // calling process's state. All unused variadic arguments are zero.
        if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0_usize, 0_usize, 0_usize, 0_usize) } != 0 {
            return Err(Failure::SetDumpability);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/core/process_memory.rs"]
mod tests;
