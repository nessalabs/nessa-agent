//! What the gateway reads about a process a resume report names: whether it
//! is there, its parent, when it started, whose it is and where it works.
//! A seam, so the rules that use it are tested without real processes.
use std::path::PathBuf;

/// One process, as the operating system describes it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessFacts {
    pub parent: u32,
    /// When it started, in the system's own units: the same pid with a
    /// different start is another process.
    pub started: u64,
    pub uid: u32,
    /// Its working folder, where the system says.
    pub cwd: Option<PathBuf>,
}

/// Reads processes.
pub trait Processes: Send + Sync {
    /// `pid` as it is now; `None` once it is gone (or cannot be read).
    fn facts(&self, pid: u32) -> Option<ProcessFacts>;
    /// The account the gateway runs as.
    fn own_uid(&self) -> u32;
}

/// The longest parent chain walked from a report's owner to the terminal.
const MAX_ANCESTRY: usize = 64;

/// Whether `pid` runs under `root` (a child, grandchild…, not `root`).
pub fn descends_from(processes: &dyn Processes, pid: u32, root: u32) -> bool {
    let mut at = pid;
    for _ in 0..MAX_ANCESTRY {
        let Some(facts) = processes.facts(at) else {
            return false;
        };
        if facts.parent == root {
            return true;
        }
        if facts.parent <= 1 || facts.parent == at {
            return false;
        }
        at = facts.parent;
    }
    false
}

/// The machine's processes.
pub struct SystemProcesses;

impl Processes for SystemProcesses {
    fn facts(&self, pid: u32) -> Option<ProcessFacts> {
        system_facts(pid)
    }

    fn own_uid(&self) -> u32 {
        #[cfg(unix)]
        {
            // SAFETY: geteuid has no preconditions and cannot fail.
            unsafe { libc::geteuid() }
        }
        #[cfg(not(unix))]
        {
            u32::MAX
        }
    }
}

#[cfg(target_os = "linux")]
fn system_facts(pid: u32) -> Option<ProcessFacts> {
    use std::os::unix::fs::MetadataExt;
    let base = PathBuf::from(format!("/proc/{pid}"));
    let stat = std::fs::read_to_string(base.join("stat")).ok()?;
    // `pid (comm) state ppid …`: comm may hold spaces and parentheses, so
    // the fields are read after its last `)`.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After comm: state is field 3, ppid 4, starttime 22 (from 1).
    let state = *fields.first()?;
    if state == "Z" || state == "X" {
        return None;
    }
    let parent = fields.get(1)?.parse().ok()?;
    let started = fields.get(19)?.parse().ok()?;
    let uid = std::fs::metadata(&base).ok()?.uid();
    let cwd = std::fs::read_link(base.join("cwd")).ok();
    Some(ProcessFacts {
        parent,
        started,
        uid,
        cwd,
    })
}

#[cfg(target_os = "macos")]
fn system_facts(pid: u32) -> Option<ProcessFacts> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: proc_bsdinfo is plain old data; proc_pidinfo writes at most
    // the size it is given and says how much it wrote.
    let info = unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let wrote = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        );
        if wrote != size {
            return None;
        }
        info
    };
    // SZOMB: a zombie has ended; only its parent has not read its status.
    if info.pbi_status == 5 {
        return None;
    }
    // SAFETY: as above, for proc_vnodepathinfo.
    let cwd = unsafe {
        let mut paths: libc::proc_vnodepathinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        let wrote = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut paths as *mut libc::proc_vnodepathinfo).cast(),
            size,
        );
        if wrote == size {
            let bytes: Vec<u8> = paths
                .pvi_cdir
                .vip_path
                .iter()
                .flatten()
                .map(|byte| byte.to_ne_bytes()[0])
                .take_while(|byte| *byte != 0)
                .collect();
            String::from_utf8(bytes)
                .ok()
                .filter(|path| path.starts_with('/'))
                .map(PathBuf::from)
        } else {
            None
        }
    };
    Some(ProcessFacts {
        parent: info.pbi_ppid,
        started: info
            .pbi_start_tvsec
            .saturating_mul(1_000_000)
            .saturating_add(info.pbi_start_tvusec),
        uid: info.pbi_uid,
        cwd,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn system_facts(_pid: u32) -> Option<ProcessFacts> {
    None
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn reads_this_process_and_its_parent() {
        let processes = SystemProcesses;
        let me = std::process::id();
        let facts = processes.facts(me).expect("this process is there");
        assert_eq!(facts.uid, processes.own_uid());
        assert_eq!(facts.cwd, std::env::current_dir().ok());
        assert!(descends_from(&processes, me, facts.parent));
        assert!(!descends_from(&processes, facts.parent, me));
        let child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("sleep starts");
        assert!(descends_from(&processes, child.id(), me));
        let mut child = child;
        child.kill().ok();
        child.wait().ok();
        assert_eq!(processes.facts(child.id()), None, "a reaped child is gone");
    }
}
