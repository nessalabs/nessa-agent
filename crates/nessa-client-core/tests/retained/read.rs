//! How one read of a paired reader's cache walks the cached conversations,
//! and how it ends: a full cache still withdraws what the gateway no longer
//! grants, and a read its budget cut short counts only if it saved something.
use super::*;

/// A walk over conversations whose answers the test sets, keeping what the
/// walk asked of each.
struct Scripted {
    answers: Vec<(&'static str, Result<Conversation, ReadFailure>)>,
    withdraw_fails: Option<ReadFailure>,
    /// Each read asked: the conversation and whether it was withdraw-only.
    asked: Vec<(String, bool)>,
    withdrawn: Vec<String>,
    spent_after: usize,
}
impl Scripted {
    fn new(answers: Vec<(&'static str, Result<Conversation, ReadFailure>)>) -> Self {
        Self {
            answers,
            withdraw_fails: None,
            asked: Vec::new(),
            withdrawn: Vec::new(),
            spent_after: usize::MAX,
        }
    }
    fn ids(&self) -> Vec<Id> {
        self.answers
            .iter()
            .map(|(id, _)| Id::new(*id).unwrap())
            .collect()
    }
}
impl EachConversation for Scripted {
    fn spent(&self) -> bool {
        self.asked.len() >= self.spent_after
    }
    fn read(&mut self, id: &Id, withdraw_only: bool) -> Result<Conversation, ReadFailure> {
        self.asked.push((id.as_str().to_owned(), withdraw_only));
        let answer = self
            .answers
            .iter()
            .find(|(each, _)| *each == id.as_str())
            .map(|(_, answer)| answer.clone())
            .unwrap();
        match answer {
            // A withdraw-only ask never reads records.
            Ok(Conversation::Read { .. }) if withdraw_only => Ok(Conversation::Unread {
                connection_kept: true,
            }),
            answer => answer,
        }
    }
    fn withdraw(&mut self, id: &Id) -> Result<(), ReadFailure> {
        if let Some(failure) = self.withdraw_fails {
            return Err(failure);
        }
        self.withdrawn.push(id.as_str().to_owned());
        Ok(())
    }
}
fn walk(scripted: &mut Scripted, held_back: Option<ReadFailure>) -> Result<bool, ReadFailure> {
    let mut withdrawn = Vec::new();
    let ids = scripted.ids();
    let result = read_each(ids, held_back, usize::MAX, scripted, &mut withdrawn);
    assert_eq!(withdrawn, scripted.withdrawn, "reported as withdrawn");
    result
}

#[test]
fn a_full_cache_still_withdraws_what_the_gateway_no_longer_grants() {
    // A is over quota; B, after it, is no longer granted; C is still granted.
    let mut scripted = Scripted::new(vec![
        ("a", Err(ReadFailure::Quota)),
        ("b", Ok(Conversation::Withdrawn)),
        ("c", Ok(Conversation::Read { complete: true })),
    ]);
    assert_eq!(walk(&mut scripted, None), Err(ReadFailure::Quota));
    assert_eq!(scripted.withdrawn, ["b"], "B is withdrawn past A's quota");
    // Once full, the rest are only asked whether they are still granted.
    assert_eq!(
        scripted.asked,
        [
            ("a".to_owned(), false),
            ("b".to_owned(), true),
            ("c".to_owned(), true)
        ]
    );
}

#[test]
fn a_full_catalogue_still_withdraws_and_quota_outranks_a_storage_failure() {
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Withdrawn)),
        ("b", Err(ReadFailure::Cache)),
        ("c", Ok(Conversation::Withdrawn)),
    ]);
    assert_eq!(
        walk(&mut scripted, Some(ReadFailure::Quota)),
        Err(ReadFailure::Quota)
    );
    assert_eq!(scripted.withdrawn, ["a", "c"]);
    assert!(scripted.asked.iter().all(|(_, only)| *only));

    // A storage failure alone is reported as such, after the withdrawals.
    let mut scripted = Scripted::new(vec![
        ("a", Err(ReadFailure::Cache)),
        ("b", Ok(Conversation::Withdrawn)),
    ]);
    assert_eq!(walk(&mut scripted, None), Err(ReadFailure::Cache));
    assert_eq!(scripted.withdrawn, ["b"]);
}

#[test]
fn a_withdrawal_that_cannot_be_written_does_not_stop_the_others() {
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Withdrawn)),
        ("b", Ok(Conversation::Read { complete: true })),
    ]);
    scripted.withdraw_fails = Some(ReadFailure::Cache);
    assert_eq!(walk(&mut scripted, None), Err(ReadFailure::Cache));
    assert_eq!(
        scripted.asked,
        [("a".to_owned(), false), ("b".to_owned(), true)]
    );
}

#[test]
fn a_damaged_cache_or_a_lost_connection_ends_the_walk_at_once() {
    for failure in [
        ReadFailure::CacheDamaged,
        ReadFailure::ResetRequired,
        ReadFailure::Unreachable,
        ReadFailure::Refused,
        ReadFailure::Stopped,
        ReadFailure::Protocol,
    ] {
        let mut scripted = Scripted::new(vec![
            ("a", Err(failure)),
            ("b", Ok(Conversation::Withdrawn)),
        ]);
        assert_eq!(walk(&mut scripted, None), Err(failure));
        assert_eq!(scripted.asked.len(), 1, "{failure:?}");
    }
}

#[test]
fn a_walk_reports_whether_every_conversation_was_read_to_the_end() {
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Read { complete: true })),
        ("b", Ok(Conversation::Withdrawn)),
    ]);
    assert_eq!(walk(&mut scripted, None), Ok(true));
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Read { complete: false })),
        ("b", Ok(Conversation::Read { complete: true })),
    ]);
    assert_eq!(walk(&mut scripted, None), Ok(false));
    // No conversation starts once the budget is spent.
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Read { complete: true })),
        ("b", Ok(Conversation::Read { complete: true })),
    ]);
    scripted.spent_after = 1;
    assert_eq!(walk(&mut scripted, None), Ok(false));
    assert_eq!(scripted.asked.len(), 1);
}

/// Row m3: a full cache stays listed full when the walk past it is then
/// stopped or loses its connection; a damaged cache or a refusal still wins.
#[test]
fn a_held_quota_outlasts_a_stop_or_a_lost_connection() {
    for cut in [ReadFailure::Stopped, ReadFailure::Unreachable] {
        let mut scripted = Scripted::new(vec![
            ("a", Err(ReadFailure::Quota)),
            ("b", Ok(Conversation::Withdrawn)),
            ("c", Err(cut)),
        ]);
        assert_eq!(
            walk(&mut scripted, None),
            Err(ReadFailure::Quota),
            "{cut:?}"
        );
        assert_eq!(scripted.withdrawn, ["b"]);
        // A catalogue held back the same way.
        let mut scripted = Scripted::new(vec![("a", Err(cut))]);
        assert_eq!(
            walk(&mut scripted, Some(ReadFailure::Quota)),
            Err(ReadFailure::Quota)
        );
    }
    for failure in [ReadFailure::CacheDamaged, ReadFailure::Refused] {
        let mut scripted = Scripted::new(vec![("a", Err(ReadFailure::Quota)), ("b", Err(failure))]);
        assert_eq!(walk(&mut scripted, None), Err(failure));
    }
}

fn progress(completed: u64, generation: u64) -> CatalogueProgress {
    let id = |text: &str| Id::new(text).unwrap();
    CatalogueProgress {
        scope: Scope::new(
            id("receiver"),
            id("origin"),
            id("stream"),
            id("incarnation"),
            id("schema"),
            id("epoch"),
        ),
        completed,
        generation,
        active: None,
    }
}

/// A cache whose catalogue progress is at the head but which notes no
/// finished walk there is walked: an earlier walk was cut short after its
/// catalogue pass, and the conversations it did not reach may no longer be
/// granted. Only a walk noted at the same head and generation settles it.
#[test]
fn a_cache_without_the_walk_marker_is_walked_at_an_unchanged_head() {
    let saved = progress(7, 3);
    assert!(!settled(Some(&saved), None, 7), "no walk noted");
    assert!(
        !settled(Some(&saved), Some((6, 3)), 7),
        "walked at an older head"
    );
    assert!(
        !settled(Some(&saved), Some((7, 2)), 7),
        "walked before the catalogue was reset"
    );
    assert!(settled(Some(&saved), Some((7, 3)), 7));
    // The gateway's head moved on: walk, whatever was noted.
    assert!(!settled(Some(&saved), Some((7, 3)), 8));
    assert!(!settled(None, Some((7, 3)), 7), "no progress at all");
}

/// Only a walk that finished is noted, and one whose note cannot be written
/// fails as the cache's failure, so it is backed off, not walked again at
/// once: the note is what the next read settles on.
#[test]
fn a_marker_that_cannot_be_written_fails_the_read() {
    assert_eq!(walked(true, || Ok(())), Ok(true));
    assert_eq!(
        walked(true, || Err(CacheError::Unavailable)),
        Err(ReadFailure::Cache)
    );
    assert_eq!(
        walked(true, || Err(CacheError::Quota)),
        Err(ReadFailure::Quota)
    );
    // An incomplete walk notes nothing.
    assert_eq!(
        walked(false, || panic!("an incomplete walk is never noted")),
        Ok(false)
    );
}

/// A read withdraws at most as many conversations as its caller allows, so
/// the evidence one read leaves stays bounded; it then ends incomplete, and
/// the next read withdraws the rest.
#[test]
fn a_read_withdraws_at_most_its_cap_and_ends_incomplete() {
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Withdrawn)),
        ("b", Ok(Conversation::Read { complete: true })),
        ("c", Ok(Conversation::Withdrawn)),
        ("d", Ok(Conversation::Withdrawn)),
    ]);
    let mut withdrawn = vec!["earlier".to_owned()];
    let ids = scripted.ids();
    assert_eq!(
        read_each(ids, None, 2, &mut scripted, &mut withdrawn),
        Ok(false)
    );
    assert_eq!(withdrawn, ["earlier", "a", "c"]);
    assert_eq!(scripted.asked.len(), 3, "d is left for the next read");
}

fn report(complete: bool) -> ReadReport {
    ReadReport {
        moved: true,
        complete,
        conversations: 1,
    }
}

#[test]
fn a_read_its_budget_cut_short_counts_only_if_it_saved_something() {
    for cut in [ReadFailure::Stopped, ReadFailure::Unreachable] {
        // Saved something: incomplete, and the next read carries on.
        assert_eq!(read_end(Err(cut), false, true, true), ReadEnd::Incomplete);
        // Saved nothing: the peer did not answer in time.
        assert_eq!(
            read_end(Err(cut), false, true, false),
            ReadEnd::Failed(ReadFailure::Unreachable)
        );
    }
    // A walk the budget stopped before any conversation, having saved nothing.
    assert_eq!(
        read_end(Ok(report(false)), false, true, false),
        ReadEnd::Failed(ReadFailure::Unreachable)
    );
    assert_eq!(
        read_end(Ok(report(false)), false, true, true),
        ReadEnd::Read(report(false))
    );
    // Unreachable before the budget was spent is unreachable, not masked.
    assert_eq!(
        read_end(Err(ReadFailure::Unreachable), false, false, true),
        ReadEnd::Failed(ReadFailure::Unreachable)
    );
    // A stop is a stop, whatever the read saw first.
    assert_eq!(
        read_end(Err(ReadFailure::Unreachable), true, true, true),
        ReadEnd::Failed(ReadFailure::Stopped)
    );
    // Up to date is synced, saved or not.
    assert_eq!(
        read_end(Ok(report(true)), false, true, false),
        ReadEnd::Read(report(true))
    );
    // Other failures pass as they are.
    assert_eq!(
        read_end(Err(ReadFailure::Quota), false, true, true),
        ReadEnd::Failed(ReadFailure::Quota)
    );
}

/// A stop reaches a read blocked on a silent peer within one slice, by the
/// flag alone: as on Windows, where shutting the socket from another thread
/// does not wake a blocked receive. The wait the caller set is far longer.
#[test]
fn a_stop_ends_a_blocked_read_without_shutting_its_socket() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    // Accepted and silent.
    let (_held, _) = listener.accept().unwrap();
    let stop = ReadStop::new();
    let reading = std::thread::spawn({
        let stop = stop.clone();
        move || {
            let mut socket = Socket::new(stream, stop);
            socket.read_timeout(Duration::from_secs(30)).unwrap();
            let started = std::time::Instant::now();
            let read = socket.read(&mut [0; 16]);
            (read.map_err(|error| error.kind()), started.elapsed())
        }
    });
    std::thread::sleep(STOP_SLICE * 4);
    // The flag only: no socket is shut.
    stop.stopped.store(true, Ordering::SeqCst);
    let (read, waited) = reading.join().unwrap();
    assert_eq!(read, Err(std::io::ErrorKind::ConnectionAborted));
    assert!(waited < Duration::from_secs(5), "{waited:?}");
}

/// A wait with no stop still lasts as long as the caller set, slice by
/// slice, and ends as the operating system's timeout would.
#[test]
fn a_sliced_wait_lasts_as_long_as_the_caller_set() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_held, _) = listener.accept().unwrap();
    let mut socket = Socket::new(stream, ReadStop::new());
    socket.read_timeout(STOP_SLICE * 5).unwrap();
    let started = std::time::Instant::now();
    let kind = socket.read(&mut [0; 16]).unwrap_err().kind();
    assert!(matches!(
        kind,
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    assert!(
        started.elapsed() >= STOP_SLICE * 4,
        "{:?}",
        started.elapsed()
    );
}
