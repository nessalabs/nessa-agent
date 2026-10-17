//! Keep the system's notices the same as the reviews that are waiting.
//!
//! The window hands over each waiting set the gateway pushes, whole. This is
//! the one place that knows which notices are up, so it diffs the set against
//! them: it posts what arrived and takes back what left. A window that
//! reloads, or a socket that dropped and sent the empty set, cannot leave a
//! notice behind, because the next set it sends is compared against what is
//! really up, not against what that page remembers. A notice the system did
//! not take is not recorded, so the next set posts it again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::domain::{NoticeAccess, NoticeId, ReviewNotice};

/// The system's notifications. Every call may block on the system, so
/// callers run them off the window thread.
pub(crate) trait NoticeCenter: Send + Sync {
    fn access(&self) -> NoticeAccess;
    /// Ask the person to allow notices, where the system asks. Answers what
    /// they chose, or what was already decided.
    fn ask(&self) -> NoticeAccess;
    /// Post a notice. `Err` when the system did not take it.
    fn post(&self, notice: &ReviewNotice) -> Result<(), String>;
    /// Take a notice back. `Err` when it may still be up.
    fn withdraw(&self, id: &NoticeId) -> Result<(), String>;
    /// Take back every notice this app left, including a previous run's
    /// where the system can find them.
    fn withdraw_all(&self);
}

/// Where opening a notice takes the person: the review's conversation. Only
/// the hosts with a notice center have one to open.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) trait NoticeOpened: Send + Sync {
    fn opened(&self, conversation: &str);
}

/// A host with no notices: nothing is shown and nothing is asked.
#[cfg(not(target_os = "linux"))]
pub(crate) struct NoNoticeCenter;

#[cfg(not(target_os = "linux"))]
impl NoticeCenter for NoNoticeCenter {
    fn access(&self) -> NoticeAccess {
        NoticeAccess::Unsupported
    }
    fn ask(&self) -> NoticeAccess {
        NoticeAccess::Unsupported
    }
    fn post(&self, _notice: &ReviewNotice) -> Result<(), String> {
        Err("this host shows no notices".to_owned())
    }
    fn withdraw(&self, _id: &NoticeId) -> Result<(), String> {
        Ok(())
    }
    fn withdraw_all(&self) {}
}

#[derive(Default)]
struct Up {
    /// The notices the system took, and what each says.
    shown: HashMap<NoticeId, ReviewNotice>,
    /// The app is leaving: nothing is posted after the last withdrawal.
    leaving: bool,
}

/// The notices the system holds for this run.
pub(crate) struct ReviewNotices {
    center: Arc<dyn NoticeCenter>,
    up: Mutex<Up>,
    /// Whether this run has asked on its own, so a prompt the person left
    /// alone is not raised again with every set.
    asked: AtomicBool,
}

impl ReviewNotices {
    /// Starts from nothing shown. A previous run's notices name reviews this
    /// run has not been told about, so they are taken back: the window's
    /// first set shows again whichever are still waiting.
    pub(crate) fn start(center: Arc<dyn NoticeCenter>) -> Self {
        center.withdraw_all();
        Self {
            center,
            up: Mutex::default(),
            asked: AtomicBool::new(false),
        }
    }

    pub(crate) fn access(&self) -> NoticeAccess {
        self.center.access()
    }

    pub(crate) fn ask(&self) -> NoticeAccess {
        self.center.ask()
    }

    /// Make the notices that are up the ones for `waiting`.
    ///
    /// A review waiting while the person has never been asked asks them,
    /// once a run, where the system has a prompt: that is the moment a
    /// notice is wanted.
    pub(crate) fn sync(&self, waiting: Vec<ReviewNotice>) {
        if !waiting.is_empty()
            && !self.asked.load(Ordering::Relaxed)
            && self.center.access() == NoticeAccess::NotAsked
        {
            self.asked.store(true, Ordering::Relaxed);
            // Outside the lock: the person may take a while to answer.
            self.center.ask();
        }
        let mut up = self.lock();
        if up.leaving {
            return;
        }
        let wanted: HashMap<NoticeId, ReviewNotice> = waiting
            .into_iter()
            .map(|notice| (notice.id.clone(), notice))
            .collect();
        let gone: Vec<NoticeId> = up
            .shown
            .keys()
            .filter(|id| !wanted.contains_key(id))
            .cloned()
            .collect();
        for id in gone {
            match self.center.withdraw(&id) {
                Ok(()) => {
                    up.shown.remove(&id);
                }
                Err(error) => {
                    crate::host_log::warn(format!("a review notice was not taken back: {error}"));
                }
            }
        }
        for (id, notice) in wanted {
            // Already up. A title that arrived later is not worth a second
            // alert for the same review.
            if up.shown.contains_key(&id) {
                continue;
            }
            match self.center.post(&notice) {
                Ok(()) => {
                    up.shown.insert(id, notice);
                }
                Err(error) => {
                    crate::host_log::warn(format!("a review notice was not shown: {error}"));
                }
            }
        }
    }

    /// Take every notice back as the app leaves, and post none after. Waits
    /// at most `within` for the system, so a hung one cannot hold up quitting.
    pub(crate) fn leave(self: &Arc<Self>, within: Duration) {
        let (done, finished) = mpsc::channel();
        let notices = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("nessa-review-notices-leave".to_owned())
            .spawn(move || {
                let mut up = notices.lock();
                up.leaving = true;
                up.shown.clear();
                notices.center.withdraw_all();
                let _ = done.send(());
            });
        if spawned.is_err() || finished.recv_timeout(within).is_err() {
            crate::host_log::warn("review notices were not all taken back before leaving");
        }
    }

    fn lock(&self) -> MutexGuard<'_, Up> {
        // The map holds no invariant a panic could break halfway.
        self.up.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingCenter {
        calls: Mutex<Vec<String>>,
        refuse: Mutex<bool>,
        access: Mutex<Option<NoticeAccess>>,
    }

    impl RecordingCenter {
        /// The calls so far, sorted: within one set the order is the map's.
        fn take(&self) -> Vec<String> {
            let mut calls = std::mem::take(&mut *self.calls.lock().unwrap());
            calls.sort();
            calls
        }
        fn record(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
    }

    impl NoticeCenter for RecordingCenter {
        fn access(&self) -> NoticeAccess {
            self.access.lock().unwrap().unwrap_or(NoticeAccess::Allowed)
        }
        fn ask(&self) -> NoticeAccess {
            self.record("ask".to_owned());
            *self.access.lock().unwrap() = Some(NoticeAccess::Allowed);
            NoticeAccess::Allowed
        }
        fn post(&self, notice: &ReviewNotice) -> Result<(), String> {
            self.record(format!(
                "post {} {}",
                notice.id.conversation(),
                notice.title
            ));
            if *self.refuse.lock().unwrap() {
                return Err("not allowed".to_owned());
            }
            Ok(())
        }
        fn withdraw(&self, id: &NoticeId) -> Result<(), String> {
            self.record(format!("withdraw {}", id.conversation()));
            if *self.refuse.lock().unwrap() {
                return Err("not taken back".to_owned());
            }
            Ok(())
        }
        fn withdraw_all(&self) {
            self.record("withdraw all".to_owned());
        }
    }

    fn notice(conversation: &str) -> ReviewNotice {
        notice_titled(conversation, "Bash")
    }

    fn notice_titled(conversation: &str, title: &str) -> ReviewNotice {
        ReviewNotice::new(
            NoticeId::new(conversation, "e", "p").unwrap(),
            title,
            "Fix the build",
        )
    }

    fn started() -> (Arc<RecordingCenter>, Arc<ReviewNotices>) {
        let center = Arc::new(RecordingCenter::default());
        let notices = Arc::new(ReviewNotices::start(center.clone()));
        assert_eq!(center.take(), ["withdraw all"]);
        (center, notices)
    }

    #[test]
    fn arrivals_are_posted_and_departures_taken_back() {
        let (center, notices) = started();
        notices.sync(vec![notice("a"), notice("b")]);
        assert_eq!(center.take(), ["post a Bash", "post b Bash"]);
        notices.sync(vec![notice("b"), notice("c")]);
        assert_eq!(center.take(), ["post c Bash", "withdraw a"]);
        notices.sync(vec![]);
        assert_eq!(center.take(), ["withdraw b", "withdraw c"]);
    }

    #[test]
    fn a_window_that_forgot_still_clears_what_is_up() {
        // A reloaded page knows nothing of what it showed before; its first
        // set still settles what the host has up.
        let (center, notices) = started();
        notices.sync(vec![notice("a")]);
        center.take();
        notices.sync(vec![notice("b")]);
        assert_eq!(center.take(), ["post b Bash", "withdraw a"]);
    }

    #[test]
    fn a_notice_already_up_is_not_posted_again() {
        let (center, notices) = started();
        notices.sync(vec![notice("a")]);
        notices.sync(vec![notice("a")]);
        notices.sync(vec![notice_titled("a", "Read")]);
        assert_eq!(center.take(), ["post a Bash"]);
    }

    #[test]
    fn what_the_system_refused_is_tried_again() {
        let (center, notices) = started();
        *center.refuse.lock().unwrap() = true;
        notices.sync(vec![notice("a")]);
        *center.refuse.lock().unwrap() = false;
        notices.sync(vec![notice("a")]);
        notices.sync(vec![notice("a")]);
        assert_eq!(center.take(), ["post a Bash", "post a Bash"]);
    }

    #[test]
    fn a_notice_not_taken_back_is_tried_again() {
        let (center, notices) = started();
        notices.sync(vec![notice("a")]);
        *center.refuse.lock().unwrap() = true;
        notices.sync(vec![]);
        *center.refuse.lock().unwrap() = false;
        notices.sync(vec![]);
        notices.sync(vec![]);
        assert_eq!(center.take(), ["post a Bash", "withdraw a", "withdraw a"]);
    }

    #[test]
    fn the_first_waiting_review_asks_where_nobody_has_been_asked() {
        let (center, notices) = started();
        *center.access.lock().unwrap() = Some(NoticeAccess::NotAsked);
        notices.sync(vec![]);
        assert_eq!(center.take(), Vec::<String>::new());
        notices.sync(vec![notice("a")]);
        assert_eq!(center.take(), ["ask", "post a Bash"]);
        notices.sync(vec![notice("b")]);
        assert_eq!(center.take(), ["post b Bash", "withdraw a"]);
    }

    #[test]
    fn a_prompt_left_alone_is_not_raised_again() {
        let center = Arc::new(RecordingCenter::default());
        let notices = ReviewNotices::start(Arc::new(IgnoredPrompt(center.clone())));
        center.take();
        notices.sync(vec![notice("a")]);
        notices.sync(vec![notice("a")]);
        assert_eq!(center.take(), ["ask", "post a Bash", "post a Bash"]);
    }

    /// Never asked, whatever the person does with the prompt.
    struct IgnoredPrompt(Arc<RecordingCenter>);

    impl NoticeCenter for IgnoredPrompt {
        fn access(&self) -> NoticeAccess {
            NoticeAccess::NotAsked
        }
        fn ask(&self) -> NoticeAccess {
            self.0.record("ask".to_owned());
            NoticeAccess::NotAsked
        }
        fn post(&self, notice: &ReviewNotice) -> Result<(), String> {
            self.0.post(notice)?;
            Err("not allowed".to_owned())
        }
        fn withdraw(&self, id: &NoticeId) -> Result<(), String> {
            self.0.withdraw(id)
        }
        fn withdraw_all(&self) {
            self.0.withdraw_all();
        }
    }

    #[test]
    fn nothing_is_posted_once_the_app_is_leaving() {
        let (center, notices) = started();
        notices.sync(vec![notice("a")]);
        notices.leave(Duration::from_secs(5));
        notices.sync(vec![notice("b")]);
        assert_eq!(center.take(), ["post a Bash", "withdraw all"]);
    }
}
