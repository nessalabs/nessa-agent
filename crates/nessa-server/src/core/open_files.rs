//! The open-file limit, raised once while the process is still one thread.
//!
//! Every live conversation holds its agent's pipes and every client holds a
//! socket, so ten thousand of either is tens of thousands of descriptors. A
//! shell or launchd usually starts a process with a soft limit far below the
//! hard one it would be allowed (1024 on most Linux systems, 256 on macOS), and
//! a gateway left there refuses its thousandth agent with `EMFILE` however
//! much memory it has. Raising the soft limit to the hard one is what any
//! process may do for itself; nothing here can raise the hard limit.
//!
//! Agents the gateway starts inherit the raised limit.

/// A process's open-file limit: the soft one it runs under and the hard one
/// it may raise that to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limit {
    pub(crate) soft: u64,
    pub(crate) hard: u64,
}

/// The operating system's open-file limit for this process.
pub(crate) trait OpenFileLimits {
    /// The current limit, or `None` when it cannot be read.
    fn read(&self) -> Option<Limit>;
    /// The highest soft limit the system accepts under `hard`.
    fn ceiling(&self, hard: u64) -> u64;
    /// Set the soft limit, keeping the hard one; whether it was accepted.
    fn set_soft(&self, soft: u64, hard: u64) -> bool;
}

/// Raise the soft open-file limit as far as this process may, and return the
/// limit it now has, or `None` when it cannot be read. A soft limit already at
/// or above the ceiling is kept, never lowered.
pub(crate) fn raise(limits: &impl OpenFileLimits) -> Option<u64> {
    let limit = limits.read()?;
    let wanted = limits.ceiling(limit.hard);
    if wanted <= limit.soft {
        return Some(limit.soft);
    }
    if limits.set_soft(wanted, limit.hard) {
        Some(wanted)
    } else {
        Some(limit.soft)
    }
}

/// This process's own limit, through `getrlimit` and `setrlimit`.
pub(crate) struct ProcessLimits;

impl OpenFileLimits for ProcessLimits {
    fn read(&self) -> Option<Limit> {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `limit` is a valid, writable rlimit for the call to fill.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
            return None;
        }
        Some(Limit {
            soft: limit.rlim_cur,
            hard: limit.rlim_max,
        })
    }

    /// macOS refuses a soft limit above `kern.maxfilesperproc` even when the
    /// hard limit is unlimited, so it is held to that.
    #[cfg(target_os = "macos")]
    fn ceiling(&self, hard: u64) -> u64 {
        let mut per_process: libc::c_int = 0;
        let mut size = std::mem::size_of::<libc::c_int>();
        // SAFETY: the name is NUL-terminated and the output buffer is an int of `size` bytes.
        let read = unsafe {
            libc::sysctlbyname(
                c"kern.maxfilesperproc".as_ptr(),
                (&mut per_process as *mut libc::c_int).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        match u64::try_from(per_process) {
            Ok(per_process) if read == 0 && per_process > 0 => hard.min(per_process),
            _ => hard.min(10_240),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn ceiling(&self, hard: u64) -> u64 {
        hard
    }

    fn set_soft(&self, soft: u64, hard: u64) -> bool {
        let raised = libc::rlimit {
            rlim_cur: soft,
            rlim_max: hard,
        };
        // SAFETY: `raised` is a valid rlimit whose soft value is within its hard one.
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) == 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::{raise, Limit, OpenFileLimits, ProcessLimits};
    use std::cell::{Cell, RefCell};

    /// A limit that answers as told, and records what it was asked to set.
    struct Substitute {
        limit: Option<Limit>,
        ceiling: u64,
        accepts: bool,
        set: RefCell<Vec<u64>>,
        reads: Cell<usize>,
    }

    impl Substitute {
        fn new(limit: Option<Limit>, ceiling: u64, accepts: bool) -> Self {
            Self {
                limit,
                ceiling,
                accepts,
                set: RefCell::default(),
                reads: Cell::new(0),
            }
        }
    }

    impl OpenFileLimits for Substitute {
        fn read(&self) -> Option<Limit> {
            self.reads.set(self.reads.get() + 1);
            self.limit
        }
        fn ceiling(&self, hard: u64) -> u64 {
            hard.min(self.ceiling)
        }
        fn set_soft(&self, soft: u64, _hard: u64) -> bool {
            self.set.borrow_mut().push(soft);
            self.accepts
        }
    }

    #[test]
    fn a_low_soft_limit_is_raised_to_the_ceiling() {
        let limits = Substitute::new(
            Some(Limit {
                soft: 256,
                hard: 1 << 20,
            }),
            10_240,
            true,
        );
        assert_eq!(raise(&limits), Some(10_240));
        assert_eq!(*limits.set.borrow(), [10_240]);
    }

    #[test]
    fn a_refused_raise_keeps_and_reports_the_soft_limit() {
        let limits = Substitute::new(
            Some(Limit {
                soft: 1024,
                hard: 4096,
            }),
            u64::MAX,
            false,
        );
        assert_eq!(raise(&limits), Some(1024));
        assert_eq!(*limits.set.borrow(), [4096]);
    }

    #[test]
    fn a_soft_limit_above_the_ceiling_is_kept_and_nothing_is_set() {
        let limits = Substitute::new(
            Some(Limit {
                soft: 1_048_575,
                hard: u64::MAX,
            }),
            10_240,
            true,
        );
        assert_eq!(raise(&limits), Some(1_048_575));
        assert!(limits.set.borrow().is_empty());
    }

    #[test]
    fn an_unreadable_limit_is_reported_and_nothing_is_set() {
        let limits = Substitute::new(None, u64::MAX, true);
        assert_eq!(raise(&limits), None);
        assert_eq!(limits.reads.get(), 1);
        assert!(limits.set.borrow().is_empty());
    }

    #[test]
    fn this_process_reaches_its_ceiling_and_raising_twice_changes_nothing() {
        let first = raise(&ProcessLimits).expect("the open-file limit is readable");
        let limit = ProcessLimits
            .read()
            .expect("the open-file limit is readable");
        assert_eq!(limit.soft, first);
        assert!(limit.soft >= ProcessLimits.ceiling(limit.hard));
        assert_eq!(raise(&ProcessLimits), Some(first));
    }
}
