//! One terminal's OSC 7577 side, on its engine thread: which reports to
//! take, whose the record is, and when its owner went.
//!
//! The terminal keeps one record (last write wins), with one preference
//! from the spec: while the record's owner runs, a report from a process
//! under it (an editor its agent opened, say) does not replace it.
use super::processes::{descends_from, Processes};
use super::resume_report::{verify, ReportAction, Verdict};
use std::collections::HashMap;

/// How long a record's owner may be gone before the record goes. A program
/// often ends a moment before the terminal that held it reports its end.
pub const OWNER_GRACE_MS: u64 = 2_000;
/// How often the record's owner is looked for.
pub const OWNER_POLL_MS: u64 = 500;
/// Pids whose last accepted time is remembered, to refuse older reports.
const REMEMBERED_PIDS: usize = 64;

/// A record the terminal took, as reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedRecord {
    pub pid: u32,
    pub started: Option<u64>,
    pub uid: u32,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub owner_cwd: Option<String>,
    pub time: u64,
}

/// What changed about the terminal's record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeChange {
    Saved(AcceptedRecord),
    Forget,
}

/// Who holds the record now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    None,
    /// A program started by a restart that has not reported yet: its pid is
    /// unknown, so any valid report from the terminal's processes is its.
    Resumed,
    Process {
        pid: u32,
        started: Option<u64>,
    },
}

pub struct ResumeTracker {
    token: String,
    /// The terminal's first process; every owner runs under it.
    root: u32,
    owner: Owner,
    gone_since_ms: Option<u64>,
    last_poll_ms: u64,
    last_time: HashMap<u32, u64>,
}

impl ResumeTracker {
    /// For a terminal whose first process is `root`, given `token`.
    /// `resumed` when a restart started the program the record is for.
    pub fn new(token: String, root: u32, resumed: bool) -> Self {
        Self {
            token,
            root,
            owner: if resumed { Owner::Resumed } else { Owner::None },
            gone_since_ms: None,
            last_poll_ms: 0,
            last_time: HashMap::new(),
        }
    }

    /// One OSC 7577 body from the terminal's output.
    pub fn report(
        &mut self,
        body: &str,
        now_ms: u64,
        processes: &dyn Processes,
    ) -> Option<ResumeChange> {
        let report = match verify(body, &self.token, now_ms / 1000) {
            Verdict::Ignored => return None,
            Verdict::Rejected { pid } => return self.reject(pid),
            Verdict::Accepted(report) => report,
        };
        if self
            .last_time
            .get(&report.pid)
            .is_some_and(|last| report.time < *last)
        {
            return None;
        }
        // The pid is checked for lifetime, not trust: it must be a process
        // of this terminal's, running as this account.
        let facts = processes.facts(report.pid).filter(|facts| {
            facts.uid == processes.own_uid() && descends_from(processes, report.pid, self.root)
        });
        let Some(facts) = facts else {
            return self.reject(Some(report.pid));
        };
        if self.last_time.len() >= REMEMBERED_PIDS && !self.last_time.contains_key(&report.pid) {
            self.last_time.clear();
        }
        self.last_time.insert(report.pid, report.time);
        let mine = match self.owner {
            Owner::None => false,
            Owner::Resumed => true,
            Owner::Process { pid, .. } => pid == report.pid,
        };
        match report.action {
            ReportAction::Clear => {
                if !mine {
                    return None;
                }
                self.owner = Owner::None;
                self.gone_since_ms = None;
                Some(ResumeChange::Forget)
            }
            ReportAction::Resume(command) => {
                if let Owner::Process { pid, started } = self.owner {
                    let alive = processes
                        .facts(pid)
                        .is_some_and(|owner| started.is_none_or(|at| owner.started == at));
                    if pid != report.pid && alive && descends_from(processes, report.pid, pid) {
                        return None;
                    }
                }
                self.owner = Owner::Process {
                    pid: report.pid,
                    started: Some(facts.started),
                };
                self.gone_since_ms = None;
                Some(ResumeChange::Saved(AcceptedRecord {
                    pid: report.pid,
                    started: Some(facts.started),
                    uid: facts.uid,
                    argv: command.argv,
                    cwd: command.cwd,
                    owner_cwd: facts.cwd.map(|cwd| cwd.to_string_lossy().into_owned()),
                    time: report.time,
                }))
            }
        }
    }

    /// A rejected report forgets its owner's record.
    fn reject(&mut self, pid: Option<u32>) -> Option<ResumeChange> {
        let forget = match (self.owner, pid) {
            (Owner::None, _) => false,
            (_, None) | (Owner::Resumed, _) => true,
            (Owner::Process { pid: owner, .. }, Some(pid)) => owner == pid,
        };
        forget.then(|| {
            self.owner = Owner::None;
            self.gone_since_ms = None;
            ResumeChange::Forget
        })
    }

    /// Look for the record's owner, at most every [`OWNER_POLL_MS`]: once
    /// it has been gone [`OWNER_GRACE_MS`], its record goes.
    pub fn poll(&mut self, now_ms: u64, processes: &dyn Processes) -> Option<ResumeChange> {
        let Owner::Process { pid, started } = self.owner else {
            return None;
        };
        if now_ms.saturating_sub(self.last_poll_ms) < OWNER_POLL_MS {
            return None;
        }
        self.last_poll_ms = now_ms;
        let alive = processes
            .facts(pid)
            .is_some_and(|facts| started.is_none_or(|at| facts.started == at));
        if alive {
            self.gone_since_ms = None;
            return None;
        }
        let since = *self.gone_since_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) < OWNER_GRACE_MS {
            return None;
        }
        self.owner = Owner::None;
        self.gone_since_ms = None;
        Some(ResumeChange::Forget)
    }
}

#[cfg(test)]
mod tests {
    use super::super::processes::ProcessFacts;
    use super::*;
    use base64::Engine;
    use sha2::{Digest, Sha256};
    use std::sync::Mutex;

    const TOKEN: &str = "a-token-of-twenty-two-chars";

    /// A process table: pid -> (parent, started, uid).
    #[derive(Default)]
    struct Table(Mutex<HashMap<u32, (u32, u64, u32)>>);

    impl Table {
        fn with(rows: &[(u32, u32)]) -> Self {
            let table = Self::default();
            for (pid, parent) in rows {
                table.0.lock().unwrap().insert(*pid, (*parent, 1, 501));
            }
            table
        }
        fn end(&self, pid: u32) {
            self.0.lock().unwrap().remove(&pid);
        }
    }

    impl Processes for Table {
        fn facts(&self, pid: u32) -> Option<ProcessFacts> {
            self.0
                .lock()
                .unwrap()
                .get(&pid)
                .map(|(parent, started, uid)| ProcessFacts {
                    parent: *parent,
                    started: *started,
                    uid: *uid,
                    cwd: Some("/work".into()),
                })
        }
        fn own_uid(&self) -> u32 {
            501
        }
    }

    fn report(pid: u32, time: u64, tail: &str) -> String {
        let body = format!("pid={pid}:time={time}:{tail}");
        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        hasher.update(b":");
        hasher.update(TOKEN.as_bytes());
        let mac: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("{body}:mac={mac}")
    }

    fn argv(parts: &[&str]) -> String {
        parts
            .iter()
            .map(|part| base64::engine::general_purpose::STANDARD.encode(part))
            .collect::<Vec<_>>()
            .join(",")
    }

    // root 10 (the terminal's first process) -> shell 11 -> claude 12 -> vim 13
    fn table() -> Table {
        Table::with(&[(10, 1), (11, 10), (12, 11), (13, 12), (99, 1)])
    }

    #[test]
    fn takes_a_report_from_its_own_processes_only() {
        let processes = table();
        let mut tracker = ResumeTracker::new(TOKEN.into(), 10, false);
        let saved = tracker.report(
            &report(12, 100, &format!("argv={}", argv(&["claude"]))),
            100_000,
            &processes,
        );
        assert!(matches!(
            saved,
            Some(ResumeChange::Saved(AcceptedRecord { pid: 12, ref owner_cwd, .. })) if owner_cwd.as_deref() == Some("/work")
        ));
        // A process outside the terminal is rejected: the record of 99 is
        // not this terminal's, so nothing is forgotten.
        assert_eq!(
            tracker.report(
                &report(99, 100, &format!("argv={}", argv(&["x"]))),
                100_000,
                &processes
            ),
            None
        );
        // An older report from the same pid is ignored.
        assert_eq!(
            tracker.report(&report(12, 99, "clear=1"), 100_000, &processes),
            None
        );
        // The owner clears; a clear from another process changes nothing.
        assert_eq!(
            tracker.report(&report(11, 100, "clear=1"), 100_000, &processes),
            None
        );
        assert_eq!(
            tracker.report(&report(12, 101, "clear=1"), 100_000, &processes),
            Some(ResumeChange::Forget)
        );
    }

    #[test]
    fn prefers_the_running_owner_over_a_process_under_it() {
        let processes = table();
        let mut tracker = ResumeTracker::new(TOKEN.into(), 10, false);
        tracker.report(
            &report(12, 100, &format!("argv={}", argv(&["claude"]))),
            100_000,
            &processes,
        );
        assert_eq!(
            tracker.report(
                &report(13, 100, &format!("argv={}", argv(&["vim"]))),
                100_000,
                &processes
            ),
            None
        );
        // Once the owner is gone, the next report takes the record.
        processes.end(12);
        processes.0.lock().unwrap().insert(13, (11, 1, 501));
        assert!(matches!(
            tracker.report(
                &report(13, 101, &format!("argv={}", argv(&["vim"]))),
                100_000,
                &processes
            ),
            Some(ResumeChange::Saved(AcceptedRecord { pid: 13, .. }))
        ));
    }

    #[test]
    fn forgets_the_record_once_its_owner_has_been_gone_a_while() {
        let processes = table();
        let mut tracker = ResumeTracker::new(TOKEN.into(), 10, false);
        tracker.report(
            &report(12, 100, &format!("argv={}", argv(&["claude"]))),
            100_000,
            &processes,
        );
        assert_eq!(tracker.poll(100_500, &processes), None);
        processes.end(12);
        assert_eq!(
            tracker.poll(101_000, &processes),
            None,
            "gone, in its grace"
        );
        assert_eq!(tracker.poll(102_000, &processes), None);
        assert_eq!(
            tracker.poll(103_000, &processes),
            Some(ResumeChange::Forget)
        );
        assert_eq!(tracker.poll(104_000, &processes), None);
    }

    #[test]
    fn a_rejected_report_forgets_its_owners_record() {
        let processes = table();
        let mut tracker = ResumeTracker::new(TOKEN.into(), 10, false);
        tracker.report(
            &report(12, 100, &format!("argv={}", argv(&["claude"]))),
            100_000,
            &processes,
        );
        assert_eq!(
            tracker.report(
                &report(12, 100, &format!("argv={}", argv(&["bin/claude"]))),
                100_000,
                &processes
            ),
            Some(ResumeChange::Forget)
        );
    }

    #[test]
    fn a_resumed_program_owns_the_record_until_it_reports() {
        let processes = table();
        let mut tracker = ResumeTracker::new(TOKEN.into(), 10, true);
        assert_eq!(tracker.poll(100_000, &processes), None);
        assert_eq!(
            tracker.report(&report(13, 100, "clear=1"), 100_000, &processes),
            Some(ResumeChange::Forget)
        );
        let mut tracker = ResumeTracker::new(TOKEN.into(), 10, true);
        assert!(matches!(
            tracker.report(
                &report(12, 100, &format!("argv={}", argv(&["claude"]))),
                100_000,
                &processes
            ),
            Some(ResumeChange::Saved(_))
        ));
    }
}
