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

/// Raise the soft open-file limit as far as this process may, and return the
/// limit it now has, or `None` when it cannot be read.
pub(crate) fn raise() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid, writable rlimit for the call to fill.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return None;
    }
    let wanted = ceiling(limit.rlim_max);
    if wanted <= limit.rlim_cur {
        return Some(limit.rlim_cur as u64);
    }
    let raised = libc::rlimit {
        rlim_cur: wanted,
        rlim_max: limit.rlim_max,
    };
    // SAFETY: `raised` is a valid rlimit whose soft value is within its hard one.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
        Some(wanted as u64)
    } else {
        Some(limit.rlim_cur as u64)
    }
}

/// The highest soft limit the kernel accepts under `hard`. macOS refuses a
/// soft limit above `kern.maxfilesperproc` even when the hard limit is
/// unlimited, so it is held to that.
#[cfg(target_os = "macos")]
fn ceiling(hard: libc::rlim_t) -> libc::rlim_t {
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
    match libc::rlim_t::try_from(per_process) {
        Ok(per_process) if read == 0 && per_process > 0 => hard.min(per_process),
        _ => hard.min(libc::rlim_t::from(10_240u32)),
    }
}

#[cfg(not(target_os = "macos"))]
fn ceiling(hard: libc::rlim_t) -> libc::rlim_t {
    hard
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_soft_limit_ends_at_the_hard_one_and_raising_twice_changes_nothing() {
        let first = super::raise().expect("the open-file limit is readable");
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `limit` is a valid, writable rlimit for the call to fill.
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
            0
        );
        assert_eq!(limit.rlim_cur as u64, first);
        assert_eq!(limit.rlim_cur, super::ceiling(limit.rlim_max));
        assert_eq!(super::raise(), Some(first));
    }
}
