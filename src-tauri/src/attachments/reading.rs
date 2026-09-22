//! Reading a chosen file's bytes: from the panel's ticket to an `ArrayBuffer`.
//!
//! The other half of the seam, and it is here because of a rule the panel
//! applies *after* the choice: what becomes of an attached file is decided by
//! the file's type, never by the gesture that attached it. An image is uploaded
//! to the gateway and normalised there; anything else is carried as a path. So
//! a file picked through the host that turns out to be an image still has to be
//! uploaded, the panel needs its bytes to upload it, and the host is the only
//! thing on this machine that can read them.
//!
//! Three things gate the read, in this order, and each one is a refusal the
//! panel can act on:
//!
//! 1. **The ticket.** The panel presents a token, not a path. A token this host
//!    did not mint, or has already spent, or has run out of time, is turned away
//!    before any filesystem is touched — and the refusal names no file, because
//!    naming one would hand back the path the ticket exists to withhold.
//! 2. **The kind.** The `stat` comes first and settles what the path names. A
//!    pipe, socket, device or directory is refused on what the `stat` said, so
//!    the `open` that would never return is never made.
//! 3. **The length.** [`LARGEST_ATTACHMENT_BYTES`] is checked before a
//!    byte is allocated, and the read is then allowed one byte past it, so a
//!    file that outgrew its own declared length is refused rather than
//!    delivered short.
//!
//! All of it runs on a thread of its own under [`super::files::LONGEST_READ_WAIT`],
//! which is what covers the two cases the `stat` cannot: a `stat` that hangs
//! before it can report anything, and a path swapped for a pipe in the moment
//! between the `stat` and the open.
//!
//! This is deliberately a *second* read of the path, separated from the choice
//! by however long the person spent looking at what they had attached. The file
//! may have been moved, replaced, truncated, grown or deleted in between, and
//! this makes no attempt to pretend otherwise: every way that can fail is a
//! typed reason rather than an assumption.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tauri::ipc::Response;
use tauri::State;

use super::files::{answered_within, ChosenFiles, Kind, OnDisk, LONGEST_READ_WAIT};
use super::refusal::{named, FileNotAttached};
use super::tickets::{AttachmentTickets, Redeemed};
use crate::composition::HostDependencies;

/// The most of one file the panel will hold, in bytes.
///
/// A ceiling on the destination, not on the disk. These bytes are on their way
/// into a webview, by way of the host's own memory and the IPC layer's, and an
/// unbounded file is several unbounded copies of itself in one process. 64 MiB
/// is comfortably more than any image somebody attaches to a message and
/// comfortably less than a number that takes a machine down.
///
/// A bound on volume, and never on reach: what the panel is allowed to read at
/// all is settled by the ticket, before this is consulted. Treating a size
/// limit as a safety property is exactly the mistake this module used to make.
pub const LARGEST_ATTACHMENT_BYTES: u64 = 64 * 1024 * 1024;

/// What the panel gets when it asks for a chosen file's bytes.
///
/// The whole decision, and pure. `most` is the bound —
/// [`LARGEST_ATTACHMENT_BYTES`] in the running app, something small in a test —
/// and `read` is a function rather than a value on purpose: "refuse before
/// allocating" is part of the rule, not something the caller is trusted to
/// remember, so a file already over the bound is refused with the read never
/// made at all.
///
/// The order is the rule. A `stat` that failed settles it first, because there
/// is nothing to bound a read by. What the `stat` says the path *is* settles it
/// next, because opening a pipe is how this whole module used to hang. A length
/// over the bound settles it next, before a byte is allocated. Only then is the
/// read asked for, and asked for one byte past the bound, so a file that is
/// longer than it claimed — it grew, or it was never the kind of thing with a
/// length worth believing — comes back over the bound and is refused as too
/// large rather than handed over short.
pub(super) fn attachment_bytes(
    path: &Path,
    looked: std::io::Result<OnDisk>,
    most: u64,
    read: impl FnOnce(u64) -> std::io::Result<Vec<u8>>,
) -> Result<Vec<u8>, FileNotAttached> {
    let shown = named(path);
    let on_disk = match looked {
        Ok(on_disk) => on_disk,
        Err(error) => return Err(FileNotAttached::size_unreadable(&shown, &error)),
    };
    if on_disk.kind != Kind::Regular {
        return Err(FileNotAttached::not_a_regular_file(
            &shown,
            on_disk.kind.described(),
        ));
    }
    if on_disk.size > most {
        return Err(FileNotAttached::file_too_large(&shown, on_disk.size, most));
    }

    let bytes = match read(most.saturating_add(1)) {
        Ok(bytes) => bytes,
        Err(error) => return Err(FileNotAttached::file_unreadable(&shown, &error)),
    };
    if bytes.len() as u64 > most {
        return Err(FileNotAttached::file_too_large(
            &shown,
            bytes.len() as u64,
            most,
        ));
    }
    Ok(bytes)
}

/// One redemption, from the ticket the panel presents to the bytes it gets.
///
/// Split from [`read_attachment_bytes`] with its outside things supplied and
/// its two bounds as parameters, so every refusal — a ticket presented twice, a
/// ticket presented too late, a pipe, a mount that never answers — is written
/// down without a filesystem, a clock, or a window server.
///
/// The ticket is spent by redeeming it, whatever the read then does. A read
/// that failed is not a second chance at the same ticket: picking the file
/// again is, which mints a new one and re-reads what is on disk *now* rather
/// than what was there when the ticket was minted.
pub(super) async fn redeemed_bytes(
    tickets: &dyn AttachmentTickets,
    files: Arc<dyn ChosenFiles>,
    ticket: &str,
    most: u64,
    wait: Duration,
) -> Result<Vec<u8>, FileNotAttached> {
    let path = match tickets.redeem(ticket) {
        Redeemed::Path(path) => path,
        Redeemed::AlreadyUsed => return Err(FileNotAttached::ticket_already_used()),
        Redeemed::Expired => return Err(FileNotAttached::ticket_expired()),
        Redeemed::Unknown => return Err(FileNotAttached::ticket_unknown()),
    };

    let shown = named(&path);
    let read = answered_within(wait, move || {
        attachment_bytes(&path, files.look(&path), most, |most| {
            files.bytes(&path, most)
        })
    })
    .await;
    match read {
        Ok(read) => read,
        Err(no_answer) => Err(FileNotAttached::no_filesystem_answer(
            &shown,
            &no_answer.detail,
        )),
    }
}

/// The bytes of one file the picker already answered with.
///
/// Takes the one-shot `ticket` the picker minted, never a path. The host
/// resolves it to a path itself, and the panel has no way to name a file this
/// host did not already agree to remember — see [`super::tickets`].
///
/// `async` because the read is a blocking one and can be 64 MiB of it; a
/// synchronous command would do that on the thread the window is drawn from,
/// and an `async` one that blocked inline would do it on a thread the whole
/// panel's other commands are waiting for.
///
/// The bytes come back as [`Response`], which Tauri sends as
/// `application/octet-stream` and the page receives as an `ArrayBuffer`. The
/// ordinary serialized path would make a JSON array of numbers out of them —
/// several bytes of text per byte of file, built and parsed twice.
#[tauri::command]
pub async fn read_attachment_bytes(
    deps: State<'_, HostDependencies>,
    ticket: String,
) -> Result<Response, FileNotAttached> {
    let tickets = deps.tickets.clone();
    redeemed_bytes(
        &*tickets,
        deps.files.clone(),
        &ticket,
        LARGEST_ATTACHMENT_BYTES,
        LONGEST_READ_WAIT,
    )
    .await
    .map(Response::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::doubles::{FakeFiles, FakeTickets};
    use crate::attachments::refusal::NotAttached;
    use std::io::{Error, ErrorKind};
    use std::path::PathBuf;

    /// A small bound, so every case below is a number a reader can hold. The
    /// shipped one is [`LARGEST_ATTACHMENT_BYTES`]; the rule does not know the
    /// difference, which is why it takes the bound as a parameter.
    const SMALL: u64 = 8;

    /// A wait long enough that nothing in these tests reaches it, so a failure
    /// here is about the decision rather than about a slow machine.
    const PATIENT: Duration = Duration::from_secs(30);

    fn path(name: &str) -> PathBuf {
        ["/Users/dev/Pictures", name].iter().collect()
    }

    fn ordinary(size: u64) -> std::io::Result<OnDisk> {
        Ok(OnDisk {
            kind: Kind::Regular,
            size,
            stored: crate::attachments::files::Stored::Locally,
        })
    }

    /// The byte read as [`read_attachment_bytes`] performs it, with the
    /// filesystem substituted and the bound made small enough to write down.
    fn read(named: &str, files: Arc<FakeFiles>) -> Result<Vec<u8>, FileNotAttached> {
        redeem(
            files,
            Arc::new(FakeTickets::for_path("a-ticket", &path(named))),
            PATIENT,
        )
    }

    fn redeem(
        files: Arc<FakeFiles>,
        tickets: Arc<FakeTickets>,
        wait: Duration,
    ) -> Result<Vec<u8>, FileNotAttached> {
        tauri::async_runtime::block_on(redeemed_bytes(&*tickets, files, "a-ticket", SMALL, wait))
    }

    /// The ordinary case, and the one fact about it that is easy to get wrong:
    /// the bytes arrive exactly as the filesystem gave them, unmodified.
    #[test]
    fn a_file_within_the_bound_is_handed_over_byte_for_byte() {
        let files = Arc::new(FakeFiles::lying(3, vec![0x89, 0x50, 0x4e]));

        assert_eq!(read("small.png", files), Ok(vec![0x89, 0x50, 0x4e]));
    }

    /// The ticket is what names the file, and the path the panel still holds is
    /// not. The desk is asked, and what it answers with is what gets read.
    #[test]
    fn the_ticket_is_what_names_the_file_and_the_panel_never_does() {
        let files = Arc::new(FakeFiles::holding(4));
        let tickets = Arc::new(FakeTickets::for_path("a-ticket", &path("holiday.heic")));

        redeem(files.clone(), tickets.clone(), PATIENT).expect("a small file is readable");

        assert_eq!(tickets.presented(), vec!["a-ticket".to_string()]);
        assert_eq!(files.asked(), vec![path("holiday.heic")]);
        assert_eq!(
            files.opened(),
            vec![(path("holiday.heic"), SMALL + 1)],
            "the path read is the one the desk resolved, not one the caller named"
        );
    }

    /// The defect this closes. A ticket the host never minted names nothing, so
    /// the filesystem is never touched — and the refusal says nothing about any
    /// file, because saying which file would be handing back the path.
    #[test]
    fn a_ticket_the_host_never_minted_reads_nothing_and_names_nothing() {
        let files = Arc::new(FakeFiles::holding(4));

        let refused = redeem(
            files.clone(),
            Arc::new(FakeTickets::refusing(Redeemed::Unknown)),
            PATIENT,
        )
        .expect_err("an unknown ticket buys no read");

        assert_eq!(refused.reason, NotAttached::TicketUnknown);
        assert_eq!(refused.shown, None);
        assert_eq!(refused.detail, None);
        assert!(
            files.asked().is_empty() && files.opened().is_empty(),
            "nothing on this machine was touched"
        );
    }

    /// Spent, expired and unknown are three answers rather than one, because
    /// the panel offers a different way out of each: a bug in its own bookkeeping,
    /// a file worth picking again, and nothing it has any record of.
    #[test]
    fn a_spent_or_expired_ticket_is_refused_as_itself_and_still_names_nothing() {
        for (redeems, reason) in [
            (Redeemed::AlreadyUsed, NotAttached::TicketAlreadyUsed),
            (Redeemed::Expired, NotAttached::TicketExpired),
        ] {
            let files = Arc::new(FakeFiles::holding(4));

            let refused = redeem(
                files.clone(),
                Arc::new(FakeTickets::refusing(redeems.clone())),
                PATIENT,
            )
            .expect_err("a ticket that is not good buys no read");

            assert_eq!(refused.reason, reason, "{redeems:?}");
            assert_eq!(refused.shown, None, "{redeems:?}");
            assert!(files.opened().is_empty(), "{redeems:?}");
        }
    }

    /// The read is allowed one byte more than the panel will keep. That extra
    /// byte is the whole mechanism by which a file longer than it claimed is
    /// noticed rather than delivered short, so it is asserted directly.
    #[test]
    fn the_read_is_allowed_one_byte_past_the_bound() {
        let files = Arc::new(FakeFiles::holding(4));

        read("photo.jpg", files.clone()).expect("a small file is readable");

        assert_eq!(files.opened(), vec![(path("photo.jpg"), SMALL + 1)]);
    }

    /// A file exactly at the bound is kept. The bound is the most the panel will
    /// hold, not the most minus one, and an off-by-one here would refuse a file
    /// the product says is fine.
    #[test]
    fn a_file_exactly_at_the_bound_is_kept() {
        let files = Arc::new(FakeFiles::holding(SMALL));

        let bytes = read("exact.png", files).expect("the bound is inclusive");

        assert_eq!(bytes.len() as u64, SMALL);
    }

    /// Gone, unmounted, or no longer readable between the choice and this
    /// second read. Reported by name, with the operating system's own words
    /// kept for the diagnostics.
    #[test]
    fn a_file_whose_length_cannot_be_read_is_refused_and_never_opened() {
        let files = Arc::new(FakeFiles::refusing(ErrorKind::NotFound));

        let refused =
            read("gone.png", files.clone()).expect_err("a file that is not there has no bytes");

        assert_eq!(refused.reason, NotAttached::SizeUnreadable);
        assert_eq!(refused.shown.as_deref(), Some("gone.png"));
        assert!(refused.detail.is_some(), "{refused:?}");
        assert!(
            files.opened().is_empty(),
            "there was no length to bound a read by, so no read was made"
        );
    }

    /// The measured bug, as a test. A named pipe answers a `stat` and never
    /// answers an `open`; it is refused on the `stat`, and the open that would
    /// have parked a thread for three minutes and forty-nine seconds is never
    /// made at all.
    #[test]
    fn a_pipe_is_refused_on_its_stat_and_never_opened() {
        let files = Arc::new(FakeFiles::being(Kind::Other));

        let refused =
            read("a-pipe", files.clone()).expect_err("a pipe has no bytes worth waiting for");

        assert_eq!(refused.reason, NotAttached::NotARegularFile);
        assert_eq!(refused.shown.as_deref(), Some("a-pipe"));
        assert!(
            files.opened().is_empty(),
            "the open that never returns was never made"
        );
    }

    /// And the same before anything is opened for a directory, which a person
    /// can plausibly have picked by accident.
    #[test]
    fn a_directory_is_refused_on_its_stat_and_never_opened() {
        let files = Arc::new(FakeFiles::being(Kind::Directory));

        let refused = read("Documents", files.clone()).expect_err("a directory has no bytes");

        assert_eq!(refused.reason, NotAttached::NotARegularFile);
        assert!(files.opened().is_empty());
    }

    /// The case the kind check cannot reach: the `stat` itself never comes
    /// back, which is what a stalled network mount does. The host stops waiting
    /// and says so, naming the file and how long it waited.
    #[test]
    fn a_filesystem_that_never_answers_is_abandoned_at_the_deadline() {
        let files = Arc::new(FakeFiles::stalling(Duration::from_secs(5)));

        let refused = redeem(
            files,
            Arc::new(FakeTickets::for_path("a-ticket", &path("holiday.heic"))),
            Duration::from_millis(50),
        )
        .expect_err("a mount that is not answering has no bytes");

        assert_eq!(refused.reason, NotAttached::FilesystemStalled);
        assert_eq!(refused.shown.as_deref(), Some("holiday.heic"));
        assert!(
            refused
                .detail
                .as_deref()
                .is_some_and(|said| said.contains("50 ms")),
            "{refused:?}"
        );
    }

    /// The file answered a `stat` and then would not open — a permission
    /// change, a volume pulled, bad media. Its own reason, because it is its
    /// own fact: this is not a file that was already missing, and not a pipe.
    #[test]
    fn a_file_that_stats_and_will_not_open_is_refused_as_unreadable() {
        let files = Arc::new(FakeFiles::unreadable(4, ErrorKind::PermissionDenied));

        let refused =
            read("locked.png", files).expect_err("a file that will not open has no bytes");

        assert_eq!(refused.reason, NotAttached::FileUnreadable);
        assert_eq!(refused.shown.as_deref(), Some("locked.png"));
        assert!(refused.detail.is_some(), "{refused:?}");
    }

    /// The bound doing its job before any memory is spent. A file that says it
    /// is larger than the panel will hold is refused on that word alone — the
    /// read is never made, which is the difference between refusing a 64 GiB
    /// file and allocating one.
    #[test]
    fn a_file_larger_than_the_bound_is_refused_before_anything_is_read() {
        let files = Arc::new(FakeFiles::lying(SMALL + 1, vec![b'n'; 64]));

        let refused =
            read("huge.tiff", files.clone()).expect_err("a file over the bound is not carried");

        assert_eq!(refused.reason, NotAttached::FileTooLarge);
        assert_eq!(refused.shown.as_deref(), Some("huge.tiff"));
        assert!(
            files.opened().is_empty(),
            "refused on its declared length, before a byte was allocated"
        );
    }

    /// And the same refusal for a file that only turns out to be too large once
    /// the reading starts: it grew between the `stat` and the open, or it never
    /// had a length worth believing. The bytes read are thrown away rather than
    /// handed over as a truncated file that would upload as a broken image.
    #[test]
    fn a_file_that_reads_longer_than_it_claimed_is_refused_as_too_large() {
        let files = Arc::new(FakeFiles::lying(4, vec![b'n'; 64]));

        let refused = read("growing.png", files.clone())
            .expect_err("a file that outgrew its own length is not carried");

        assert_eq!(refused.reason, NotAttached::FileTooLarge);
        // The read stopped at the bound plus one rather than running to the end
        // of a file with no end worth trusting.
        assert_eq!(files.opened(), vec![(path("growing.png"), SMALL + 1)]);
        assert_eq!(
            refused.detail.as_deref(),
            Some("9 bytes, over the 8 the panel will hold"),
            "the diagnostic says what was read, not what was claimed"
        );
    }

    /// The two too-large cases are one reason on screen and two different
    /// sentences in the log, so a file that lied can still be told apart from
    /// one that was simply big.
    #[test]
    fn a_declared_size_and_a_discovered_one_are_reported_differently() {
        let declared = read("big.png", Arc::new(FakeFiles::lying(20, vec![b'n'; 20])))
            .expect_err("over the bound");
        let discovered = read("big.png", Arc::new(FakeFiles::lying(1, vec![b'n'; 20])))
            .expect_err("over the bound once read");

        assert_eq!(declared.reason, discovered.reason);
        assert_ne!(declared.detail, discovered.detail);
    }

    /// A path with no final component still names the file it is refusing, by
    /// falling back to the path itself. A refusal nobody can attach to a file is
    /// no better than silence.
    #[test]
    fn a_path_with_no_final_component_is_still_pointed_at() {
        let refused = attachment_bytes(
            Path::new("/"),
            Err(Error::from(ErrorKind::NotFound)),
            SMALL,
            |_| unreachable!("there was no length to bound a read by"),
        )
        .expect_err("a root is not a file");

        assert_eq!(refused.shown.as_deref(), Some("/"));
    }

    /// An empty file is a file. It reads as no bytes rather than as a failure,
    /// because "there is nothing in it" and "it could not be read" are answers
    /// the panel acts on differently.
    #[test]
    fn an_empty_file_reads_as_no_bytes_rather_than_a_refusal() {
        let files = Arc::new(FakeFiles::holding(0));

        assert_eq!(read("empty.png", files), Ok(Vec::new()));
    }

    /// The decision is the same whatever bound it is given, which is what makes
    /// the shipped 64 MiB the same rule these tests exercise at eight bytes.
    #[test]
    fn the_shipped_bound_is_the_same_rule_at_a_different_number() {
        let refused = attachment_bytes(
            &path("enormous.tiff"),
            ordinary(LARGEST_ATTACHMENT_BYTES + 1),
            LARGEST_ATTACHMENT_BYTES,
            |_| unreachable!("a file over the bound is never read"),
        )
        .expect_err("over the shipped bound");

        assert_eq!(refused.reason, NotAttached::FileTooLarge);
    }
}
