//! Choosing files: from the person's click to what the panel is handed.
//!
//! One trip through here asks four outside things in a fixed order — the
//! picker, the filesystem, the platform's type database, the ticket desk — and
//! everything that decides anything is [`attachment`], which takes their
//! answers as parameters and reaches for nothing.
//!
//! The order is the rule, and each step is there because the one after it would
//! be wrong without it. A path that is not text is refused first: there is no
//! name to report anything else against. A path that names no file is refused
//! next. Then the `stat`, then what the `stat` says the thing *is* — because a
//! named pipe answers a `stat` and never answers an `open`, so the kind has to
//! settle it before anything opens anything. Only a path that has survived all
//! four is worth a ticket, which is why the ticket is minted last and by a
//! closure rather than handed in: a file about to be refused must not leave a
//! ticket behind in the host.
//!
//! One unusable file refuses the whole selection, because a person who chose
//! five files and silently got four has been told nothing about the fifth.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::State;

use super::content_type::ContentTypes;
use super::files::{answered_within, ChosenFiles, Kind, OnDisk, Stored, LONGEST_LOOK_WAIT};
use super::picker::{FilePicker, Picked};
use super::readiness::{Announce, Readied, Readiness, Telling, LONGEST_READY_WAIT, READY_POLL};
use super::refusal::{named, FileNotAttached, NotAttached};
use super::tickets::{AttachmentTickets, NoTicket};
use crate::composition::HostDependencies;

/// One file the person chose, as the panel receives it.
///
/// `path` is what an agent on this machine opens, `name` is what the person
/// reads, `size` is how many bytes are there, `mime_type` is what this machine
/// says the file is, and `ticket` is the one-shot token that reads it. All five
/// come from the host rather than from the page, which is the point of asking
/// the host at all.
///
/// `mime_type` and `ticket` are the two that are easy to misread. `mime_type`
/// goes exactly where a browser's `File.type` goes, so a file picked here and
/// the same file dropped on the panel take the same route — see
/// [`super::content_type`]. `ticket` is what reads the file, and `path` is not:
/// the path is in the answer because the gateway is sent it, and it has stopped
/// being what authorises anything — see [`super::tickets`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChosenFile {
    /// The absolute path, as the operating system spells it.
    pub path: String,
    /// The final component of that path.
    pub name: String,
    /// The file's length in bytes.
    pub size: u64,
    /// What this machine says the file is, lowercase and without parameters, or
    /// empty where the platform has no answer.
    pub mime_type: String,
    /// The one-shot ticket [`super::read_attachment_bytes`] reads this file
    /// with. Opaque, unguessable, good once.
    pub ticket: String,
}

/// What the panel is told about one chosen path.
///
/// The whole decision, and pure: it takes the path and what the four outside
/// things said about it, so every refusal — including the ones nothing can be
/// arranged to produce on demand — is one line in a test.
///
/// `ticket` is a function rather than a value because minting has an effect
/// that outlives this call: the host remembers a path for every ticket it
/// mints, and a file that is about to be refused must not leave one behind. See
/// the module header for why the rest of the order is what it is.
pub(super) fn attachment(
    path: &Path,
    looked: std::io::Result<OnDisk>,
    mime: String,
    ticket: impl FnOnce() -> Result<String, NoTicket>,
) -> Result<ChosenFile, FileNotAttached> {
    let Some(text) = path.to_str() else {
        return Err(FileNotAttached::path_not_text(path));
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(FileNotAttached::path_names_no_file(text));
    };
    let on_disk = match looked {
        Ok(on_disk) => on_disk,
        Err(error) => return Err(FileNotAttached::size_unreadable(name, &error)),
    };
    if on_disk.kind != Kind::Regular {
        return Err(FileNotAttached::not_a_regular_file(
            name,
            on_disk.kind.described(),
        ));
    }
    // And whether there is anything behind the length, which is the same shape
    // of question as the kind and asked in the same breath: both are settled
    // from the `stat`, before anything is opened, because opening is what goes
    // wrong. A placeholder either fails the read or blocks on a download for as
    // long as the download takes — and for a linked file neither of those
    // happens here at all. It happens inside the agent, later, with nothing on
    // screen to connect it to the file somebody attached.
    if on_disk.stored != Stored::Locally {
        return Err(FileNotAttached::file_not_readable(
            name,
            on_disk.stored.described(),
        ));
    }
    let ticket = match ticket() {
        Ok(ticket) => ticket,
        Err(refused) => return Err(FileNotAttached::ticket_unavailable(name, &refused.detail)),
    };

    Ok(ChosenFile {
        path: text.to_string(),
        name: name.to_string(),
        size: on_disk.size,
        mime_type: mime,
        ticket,
    })
}

/// One trip to the picker, from the person's choice to what the panel gets.
///
/// Split from [`choose_attachment_files`] with its outside things supplied, so
/// the order and every refusal survive without a window server: a cancellation
/// is an empty answer rather than an error, and one unusable file refuses the
/// whole selection rather than shortening it silently.
///
/// The `stat` and the type lookup for each chosen path happen together, on a
/// thread of its own, under `wait` — see [`answered_within`]. Per file rather
/// than per selection, and the reason is the refusal: a selection-wide deadline
/// could only report that *something* stalled, and "one of the files you chose
/// is on a disk that is not answering" is not a sentence anybody can act on.
#[allow(clippy::too_many_arguments)]
pub(super) async fn chosen_attachments(
    picker: &dyn FilePicker,
    files: Arc<dyn ChosenFiles>,
    types: Arc<dyn ContentTypes>,
    tickets: Arc<dyn AttachmentTickets>,
    readiness: Arc<Readiness>,
    announce: &dyn Announce,
    wait: Duration,
    ready_wait: Duration,
) -> Result<Vec<ChosenFile>, FileNotAttached> {
    let paths = match picker.choose().await {
        Picked::Cancelled => return Ok(Vec::new()),
        Picked::Unavailable(detail) => return Err(FileNotAttached::picker_unavailable(&detail)),
        Picked::Files(paths) => paths,
    };

    describe_each(
        paths, files, types, tickets, readiness, announce, wait, ready_wait,
    )
    .await
}

/// Describe every path in `paths`, in order, or refuse the lot.
///
/// Shared by the picker and by [`super::dropping`], and shared deliberately:
/// the whole product rule is that a file's type decides its route and the
/// gesture never does, so a file dropped on the panel and the same file
/// chosen with `+` must be described by the same code asking the same four
/// outside things in the same order. Two loops would be two chances to
/// disagree, which is the defect this feature has already had once.
#[allow(clippy::too_many_arguments)]
pub(super) async fn describe_each(
    paths: Vec<std::path::PathBuf>,
    files: Arc<dyn ChosenFiles>,
    types: Arc<dyn ContentTypes>,
    tickets: Arc<dyn AttachmentTickets>,
    readiness: Arc<Readiness>,
    announce: &dyn Announce,
    wait: Duration,
    ready_wait: Duration,
) -> Result<Vec<ChosenFile>, FileNotAttached> {
    let mut attached = Vec::with_capacity(paths.len());
    for path in paths {
        // `mut` only where something can change it: off macOS nothing is ever
        // a placeholder, so nothing is ever described twice.
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut described = describe_one(&path, &files, &types, &tickets, wait).await?;
        // A placeholder is fetched rather than refused. Somebody chose this
        // file; sending them to Finder to open it by hand is the computer
        // declining to do something it can do. What it may not do is wait
        // without a bound — see `downloads` for why that is the stalled-mount
        // hang in different clothes — so the wait is bounded and giving up is
        // an honest sentence rather than a generic failure.
        if matches!(&described, Err(refused) if refused.reason == NotAttached::FileNotReadable) {
            // Said before the wait rather than after it, which is the whole
            // point: the answer comes up to `ready_wait` later, and a panel
            // that says nothing for that long reads as broken. Told in every
            // case below, including the two that refuse, so no tile is ever
            // left waiting for a file that is not coming.
            let waiting = named(&path);
            announce.readying(&waiting);
            let readied = readiness.make_ready(&path, ready_wait, READY_POLL).await;
            announce.settled(&waiting);
            match readied {
                // Here now, so this is an ordinary local file and is described
                // again from scratch: nothing downstream knows it was ever a
                // placeholder, and the type decides its route as it would for
                // any other file.
                #[cfg(target_os = "macos")]
                Readied::Ready => {
                    described = describe_one(&path, &files, &types, &tickets, wait).await?
                }
                #[cfg(target_os = "macos")]
                Readied::StillComing => {
                    return Err(FileNotAttached::file_not_ready_yet(
                        &named(&path),
                        ready_wait,
                    ))
                }
                // Another provider's placeholder, or iCloud refusing. The
                // original refusal already says the useful thing — open it once
                // — and its detail is replaced with what the service said.
                Readied::Refused(detail) => {
                    return Err(FileNotAttached::file_not_readable(&named(&path), &detail))
                }
            }
        }
        attached.push(described?);
    }
    Ok(attached)
}

/// One path, described under the filesystem deadline.
///
/// The outer `Result` is the deadline and the inner one is the decision, kept
/// apart because the caller acts on them differently: a stalled disk ends the
/// whole selection, while a placeholder is something to go and fetch.
async fn describe_one(
    path: &std::path::Path,
    files: &Arc<dyn ChosenFiles>,
    types: &Arc<dyn ContentTypes>,
    tickets: &Arc<dyn AttachmentTickets>,
    wait: Duration,
) -> Result<Result<ChosenFile, FileNotAttached>, FileNotAttached> {
    let files = files.clone();
    let types = types.clone();
    let tickets = tickets.clone();
    let asked = path.to_path_buf();
    answered_within(wait, move || {
        attachment(&asked, files.look(&asked), types.of(&asked), || {
            tickets.mint(&asked)
        })
    })
    .await
    .map_err(|no_answer| FileNotAttached::no_filesystem_answer(&named(path), &no_answer.detail))
}

/// The files the person chooses, or why there are none.
///
/// Takes nothing: what may be attached is the host's question, not the page's.
/// An empty array is the ordinary cancellation — somebody opened the picker and
/// changed their mind — and is not a failure.
#[tauri::command]
pub async fn choose_attachment_files(
    app: tauri::AppHandle,
    deps: State<'_, HostDependencies>,
) -> Result<Vec<ChosenFile>, FileNotAttached> {
    let picker = deps.picker.clone();
    chosen_attachments(
        &*picker,
        deps.files.clone(),
        deps.types.clone(),
        deps.tickets.clone(),
        deps.readiness.clone(),
        &Telling(&app),
        LONGEST_LOOK_WAIT,
        LONGEST_READY_WAIT,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::attachments::doubles::FakeReadiness;
    use crate::attachments::doubles::{FakeFiles, FakePicker, FakeTickets, FakeTypes};
    #[cfg(target_os = "macos")]
    use crate::attachments::readiness::Readied;
    use crate::attachments::readiness::{Announce, Readiness, Untold};

    /// Everything the panel was told, in order.
    #[derive(Default)]
    struct Recording(std::sync::Mutex<Vec<(String, String)>>);

    impl Recording {
        fn said(&self) -> Vec<(String, String)> {
            self.0.lock().expect("what was said").clone()
        }
        fn push(&self, what: &str, name: &str) {
            self.0
                .lock()
                .expect("what was said")
                .push((what.to_string(), name.to_string()));
        }
    }

    impl Announce for Recording {
        fn readying(&self, name: &str) {
            self.push("readying", name);
        }
        fn settled(&self, name: &str) {
            self.push("settled", name);
        }
    }

    /// A seam with no handler at all, which is what every test but the
    /// placeholder ones wants: nothing they attach is ever not ready, so there
    /// is nothing to answer. Also exactly the shape a platform without a
    /// readiness handler ships.
    fn nothing_to_ready() -> Arc<Readiness> {
        Arc::new(Readiness::new(Vec::new()))
    }

    /// A readiness seam holding one staged handler, which is what the caller
    /// takes: the dispatch is tested beside the seam, not here.
    #[cfg(target_os = "macos")]
    fn staged(answer: Readied) -> Arc<Readiness> {
        staged_with(FakeReadiness::answering(answer))
    }
    #[cfg(target_os = "macos")]
    fn staged_with(handler: FakeReadiness) -> Arc<Readiness> {
        Arc::new(Readiness::new(vec![Arc::new(handler)]))
    }
    use crate::attachments::refusal::NotAttached;
    use crate::attachments::tickets::Redeemed;
    use std::io::{Error, ErrorKind};
    use std::path::PathBuf;

    /// A wait long enough that nothing in these tests reaches it, so a failure
    /// here is about the decision rather than about a slow machine.
    const PATIENT: Duration = Duration::from_secs(30);

    fn describe(
        path: &Path,
        looked: std::io::Result<OnDisk>,
        mime: &str,
    ) -> Result<ChosenFile, FileNotAttached> {
        attachment(
            path,
            looked,
            mime.to_string(),
            || Ok("a-ticket".to_string()),
        )
    }

    fn ordinary(size: u64) -> std::io::Result<OnDisk> {
        Ok(OnDisk {
            kind: Kind::Regular,
            size,
            stored: Stored::Locally,
        })
    }

    fn choose(
        picked: Picked,
        files: Arc<FakeFiles>,
        types: &'static str,
        tickets: Arc<FakeTickets>,
    ) -> Result<Vec<ChosenFile>, FileNotAttached> {
        tauri::async_runtime::block_on(chosen_attachments(
            &FakePicker(picked),
            files,
            Arc::new(FakeTypes(types)),
            tickets,
            nothing_to_ready(),
            &Untold,
            PATIENT,
            Duration::from_millis(50),
        ))
    }

    /// The same, with a readiness handler staged. Only a platform that has
    /// one can be asked what it would answer.
    #[cfg(target_os = "macos")]
    fn readying(
        picked: Picked,
        files: Arc<FakeFiles>,
        types: &'static str,
        tickets: Arc<FakeTickets>,
        handler: FakeReadiness,
    ) -> Result<Vec<ChosenFile>, FileNotAttached> {
        tauri::async_runtime::block_on(chosen_attachments(
            &FakePicker(picked),
            files,
            Arc::new(FakeTypes(types)),
            tickets,
            staged_with(handler),
            &Untold,
            PATIENT,
            // Short, because every outcome here is staged rather than waited
            // for; the seam owns the deadline's own test.
            Duration::from_millis(50),
        ))
    }

    /// The everyday selection, with the four outside things all answering.
    fn picking(picked: Picked, files: Arc<FakeFiles>) -> Result<Vec<ChosenFile>, FileNotAttached> {
        choose(
            picked,
            files,
            "text/markdown",
            Arc::new(FakeTickets::for_path("a-ticket", Path::new("/unused"))),
        )
    }

    /// A path that is not text at all, which is what makes the refusal real
    /// rather than theoretical: on macOS and Linux a path is bytes.
    #[cfg(unix)]
    fn unnameable() -> PathBuf {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        PathBuf::from(OsStr::from_bytes(b"/Users/dev/Pictures/\xff\xfe.png"))
    }

    #[test]
    fn an_ordinary_file_is_described_by_everything_the_host_was_able_to_learn() {
        let path: PathBuf = ["/Users/dev/Documents", "notes.md"].iter().collect();

        assert_eq!(
            describe(&path, ordinary(1_024), "text/markdown"),
            Ok(ChosenFile {
                path: path.to_str().expect("an ASCII path is text").to_string(),
                name: "notes.md".to_string(),
                size: 1_024,
                mime_type: "text/markdown".to_string(),
                ticket: "a-ticket".to_string(),
            })
        );
    }

    /// The defect this field exists for. The host carries the platform's answer
    /// through untouched, so a format the panel's extension table has never
    /// heard of is still an image by the time the panel routes it — and takes
    /// the same route it would have taken if it had been dropped on the panel.
    #[test]
    fn the_platforms_own_type_is_carried_through_unchanged() {
        for answer in ["image/vnd.microsoft.icon", "image/jp2", "image/x-targa"] {
            let described = describe(
                Path::new("/Users/dev/Pictures/icon.ico"),
                ordinary(9),
                answer,
            )
            .expect("an ordinary file is attachable");

            assert_eq!(described.mime_type, answer);
        }
    }

    /// And a platform with no answer says nothing rather than guessing. The
    /// empty string is what hands the decision to the panel's extension
    /// fallback; a type invented here would make the fallback a second source
    /// of truth that could disagree.
    #[test]
    fn a_platform_with_no_answer_yields_an_empty_type_rather_than_a_guess() {
        let described = describe(
            Path::new("/Users/dev/Pictures/holiday.heic"),
            ordinary(9),
            "",
        )
        .expect("an ordinary file is attachable");

        assert_eq!(described.mime_type, "");
    }

    /// A name is not ASCII, and is carried exactly as the filesystem spells it.
    /// Trimming, normalizing or transliterating it would produce a path that
    /// opens nothing.
    #[test]
    fn a_unicode_name_is_carried_through_unchanged() {
        let path: PathBuf = ["/Users/dev/Documents", "résumé — 履歴書.pdf"]
            .iter()
            .collect();

        let described =
            describe(&path, ordinary(12), "application/pdf").expect("a unicode name is still text");

        assert_eq!(described.name, "résumé — 履歴書.pdf");
        assert!(
            described.path.ends_with("résumé — 履歴書.pdf"),
            "{described:?}"
        );
    }

    /// The refusal this feature exists to make: a path Nessa cannot name is
    /// refused rather than converted lossily and handed over as if it opened
    /// something.
    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_text_is_refused_and_says_which_file_it_was() {
        let refused = describe(&unnameable(), ordinary(7), "image/png")
            .expect_err("bytes are not a path we can name");

        assert_eq!(refused.reason, NotAttached::PathNotText);
        let shown = refused.shown.expect("the person is told which file");
        assert!(shown.contains("Pictures"), "{shown}");
        assert!(shown.ends_with(".png"), "{shown}");
    }

    /// And it is refused whatever the filesystem said, because there is no name
    /// to report a size failure against. The two facts are read together rather
    /// than each on its own.
    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_text_is_refused_as_that_even_when_its_size_is_unreadable() {
        let refused = describe(
            &unnameable(),
            Err(Error::from(ErrorKind::NotFound)),
            "image/png",
        )
        .expect_err("bytes are not a path we can name");

        assert_eq!(refused.reason, NotAttached::PathNotText);
        assert_eq!(
            refused.detail, None,
            "there is no size failure to report yet"
        );
    }

    /// Text, and still no file: refused rather than given an invented name.
    #[test]
    fn a_path_with_no_final_component_names_no_file() {
        let refused = describe(Path::new("/"), ordinary(0), "inode/directory")
            .expect_err("a root is not a file");

        assert_eq!(refused.reason, NotAttached::PathNamesNoFile);
        assert_eq!(refused.shown.as_deref(), Some("/"));
    }

    /// The file went away between the click and the read. It is named, because
    /// by then the name is known, and the operating system's own words are kept
    /// for the diagnostics.
    #[test]
    fn a_file_whose_size_cannot_be_read_is_refused_by_name() {
        let path: PathBuf = ["/Users/dev/Documents", "gone.pdf"].iter().collect();

        let refused = describe(&path, Err(Error::from(ErrorKind::NotFound)), "")
            .expect_err("a file that is not there has no size");

        assert_eq!(refused.reason, NotAttached::SizeUnreadable);
        assert_eq!(refused.shown.as_deref(), Some("gone.pdf"));
        assert!(refused.detail.is_some(), "{refused:?}");
    }

    /// The refusal that ends the hang. A named pipe answers a `stat` perfectly
    /// well and would never answer an `open`, so it is turned away on what the
    /// `stat` said — and the person is told what it actually was.
    #[test]
    fn a_path_that_is_not_an_ordinary_file_is_refused_on_what_the_stat_said() {
        let path: PathBuf = ["/Users/dev", "a-pipe"].iter().collect();

        let refused = describe(
            &path,
            Ok(OnDisk {
                kind: Kind::Other,
                size: 0,
                stored: Stored::Locally,
            }),
            "",
        )
        .expect_err("a pipe is not an attachment");

        assert_eq!(refused.reason, NotAttached::NotARegularFile);
        assert_eq!(refused.shown.as_deref(), Some("a-pipe"));
        assert!(
            refused
                .detail
                .as_deref()
                .is_some_and(|said| said.contains("pipe")),
            "{refused:?}"
        );
    }

    /// A directory is its own sentence. Somebody can plausibly have meant to
    /// pick one, and cannot plausibly have meant to pick a socket.
    #[test]
    fn a_directory_is_refused_as_a_directory() {
        let path: PathBuf = ["/Users/dev", "Documents"].iter().collect();

        let refused = describe(
            &path,
            Ok(OnDisk {
                kind: Kind::Directory,
                size: 96,
                stored: Stored::Locally,
            }),
            "inode/directory",
        )
        .expect_err("a directory is not an attachment");

        assert_eq!(refused.reason, NotAttached::NotARegularFile);
        assert!(
            refused
                .detail
                .as_deref()
                .is_some_and(|said| said.contains("directory")),
            "{refused:?}"
        );
    }

    /// The case a real user hit. `~/Library/Mobile Documents/…/amica-document
    /// 2.pdf` was an iCloud placeholder: `blocks=0`, `size=34890`, a perfectly
    /// good path with nothing behind it. Nessa linked it, the agent tried to
    /// read it, and a string of opaque failed tool calls was the only sign.
    ///
    /// Now the seam is asked, the bytes arrive, and the file is described
    /// again from scratch — so what the panel finally gets is an ordinary local
    /// file with a ticket, and nothing about it says it was ever elsewhere.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_placeholder_is_made_ready_and_then_attached_like_any_other_file() {
        let path: PathBuf = ["/Users/dev/iCloud", "amica-document 2.pdf"]
            .iter()
            .collect();
        // The filesystem says placeholder the first time and ordinary the
        // second, which is what a landed download looks like from here.
        let files = Arc::new(FakeFiles::arriving(34_890));

        let attached = readying(
            Picked::Files(vec![path.clone()]),
            files.clone(),
            "application/pdf",
            Arc::new(FakeTickets::for_path("a-ticket", Path::new("/unused"))),
            FakeReadiness::answering(Readied::Ready),
        )
        .expect("a placeholder that arrived is an ordinary file");

        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].name, "amica-document 2.pdf");
        assert_eq!(attached[0].size, 34_890);
        assert_eq!(attached[0].mime_type, "application/pdf");
        assert!(!attached[0].ticket.is_empty());
        // Looked at twice: once to find the placeholder, once after it landed.
        assert_eq!(files.asked(), vec![path.clone(), path]);
    }

    /// What the panel is told, and when. The answer arrives up to
    /// `LONGEST_READY_WAIT` after the wait begins, so a panel that only learns
    /// from the answer says nothing for forty-five seconds — which is the
    /// silence this closes. Said before the wait, and said again after it in
    /// every case including the refusals, so no tile is left waiting for a
    /// file that is not coming.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_panel_is_told_before_the_wait_and_again_when_it_ends() {
        for answer in [
            Readied::Ready,
            Readied::StillComing,
            Readied::Refused("not ours".to_string()),
        ] {
            let told = Recording::default();
            let _ = tauri::async_runtime::block_on(chosen_attachments(
                &FakePicker(Picked::Files(vec![PathBuf::from(
                    "/Users/dev/iCloud/amica-document 2.pdf",
                )])),
                Arc::new(FakeFiles::arriving(34_890)),
                Arc::new(FakeTypes("application/pdf")),
                Arc::new(FakeTickets::for_path("a-ticket", Path::new("/unused"))),
                staged(answer.clone()),
                &told,
                PATIENT,
                Duration::from_millis(50),
            ));

            assert_eq!(
                told.said(),
                vec![
                    ("readying".to_string(), "amica-document 2.pdf".to_string()),
                    ("settled".to_string(), "amica-document 2.pdf".to_string()),
                ],
                "{answer:?}"
            );
        }
    }

    /// And an ordinary file is never announced at all: a tile that appeared
    /// for every attachment would be noise, and the common path must not pay
    /// for the uncommon one.
    #[test]
    fn an_ordinary_file_is_never_announced() {
        let told = Recording::default();
        let attached = tauri::async_runtime::block_on(chosen_attachments(
            &FakePicker(Picked::Files(vec![PathBuf::from("/Users/dev/notes.md")])),
            Arc::new(FakeFiles::holding(12)),
            Arc::new(FakeTypes("text/markdown")),
            Arc::new(FakeTickets::for_path("a-ticket", Path::new("/unused"))),
            nothing_to_ready(),
            &told,
            PATIENT,
            Duration::from_millis(50),
        ))
        .expect("an ordinary file");

        assert_eq!(attached.len(), 1);
        assert!(told.said().is_empty(), "{:?}", told.said());
    }

    /// A file already here never reaches the seam at all. The common path must
    /// not pay for the uncommon one.
    ///
    /// Only where there is a handler to consult: a platform without one has an
    /// empty seam, and "it asked nobody" is true of it by construction.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_ordinary_file_never_asks_anybody_to_make_it_ready() {
        let readiness = FakeReadiness::answering(Readied::Ready);
        let asked = std::sync::Arc::new(readiness);

        let attached = tauri::async_runtime::block_on(chosen_attachments(
            &FakePicker(Picked::Files(vec![PathBuf::from("/Users/dev/notes.md")])),
            Arc::new(FakeFiles::holding(12)),
            Arc::new(FakeTypes("text/markdown")),
            Arc::new(FakeTickets::for_path("a-ticket", Path::new("/unused"))),
            Arc::new(Readiness::new(vec![asked.clone()])),
            &Untold,
            PATIENT,
            Duration::from_millis(50),
        ))
        .expect("an ordinary file");

        assert_eq!(attached.len(), 1);
        assert!(
            asked.asked().is_empty(),
            "the seam was asked about a local file"
        );
    }

    /// The deadline, seen from the caller: a file still coming when the time
    /// runs out is refused with its own reason, not a generic failure, so the
    /// panel can say "still on its way, try again" rather than "it broke".
    #[cfg(target_os = "macos")]
    #[test]
    fn a_file_still_on_its_way_at_the_deadline_is_refused_as_that() {
        let refused = readying(
            Picked::Files(vec![PathBuf::from("/Users/dev/iCloud/report.pdf")]),
            Arc::new(FakeFiles::dataless(34_890)),
            "application/pdf",
            Arc::new(FakeTickets::for_path("a-ticket", Path::new("/unused"))),
            FakeReadiness::answering(Readied::StillComing),
        )
        .expect_err("a file that has not arrived cannot be attached");

        assert_eq!(refused.reason, NotAttached::FileNotReadyYet);
        assert_eq!(refused.shown.as_deref(), Some("report.pdf"));
    }

    /// And a placeholder nothing can make ready — another provider's, or one
    /// on a platform with no iCloud — keeps the reason that tells somebody to
    /// open it once, with the service's own words in the diagnostics.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_placeholder_nothing_can_fetch_is_refused_with_what_the_service_said() {
        let refused = readying(
            Picked::Files(vec![PathBuf::from("/Users/dev/Dropbox/report.pdf")]),
            Arc::new(FakeFiles::dataless(34_890)),
            "application/pdf",
            Arc::new(FakeTickets::for_path("a-ticket", Path::new("/unused"))),
            FakeReadiness::answering(Readied::Refused("not ours to fetch".to_string())),
        )
        .expect_err("a placeholder nobody can fetch");

        assert_eq!(refused.reason, NotAttached::FileNotReadable);
        assert_eq!(refused.shown.as_deref(), Some("report.pdf"));
        assert_eq!(refused.detail.as_deref(), Some("not ours to fetch"));
    }

    /// Nothing is minted for a file that is being refused. A ticket left behind
    /// for a path nobody can attach is a path the host keeps for ten minutes
    /// for no reason at all.
    #[test]
    fn a_refused_file_leaves_no_ticket_behind() {
        let mut minted = 0;
        let refused = attachment(
            Path::new("/Users/dev/a-pipe"),
            Ok(OnDisk {
                kind: Kind::Other,
                size: 0,
                stored: Stored::Locally,
            }),
            String::new(),
            || {
                minted += 1;
                Ok("a-ticket".to_string())
            },
        );

        assert!(refused.is_err(), "{refused:?}");
        assert_eq!(minted, 0, "a refused file was never worth a ticket");
    }

    /// A host with no randomness behind it cannot mint, so the selection is
    /// refused rather than handed over with a ticket nothing can redeem. The
    /// whole way through, with the desk substituted, because a refusal the
    /// orchestration swallowed would look exactly like a successful pick until
    /// somebody tried to read a file.
    #[test]
    fn a_selection_the_host_cannot_ticket_is_refused_rather_than_handed_over() {
        let path: PathBuf = ["/Users/dev/Pictures", "holiday.heic"].iter().collect();

        let refused = choose(
            Picked::Files(vec![path]),
            Arc::new(FakeFiles::holding(12)),
            "image/heic",
            Arc::new(FakeTickets::unavailable()),
        )
        .expect_err("a file with no ticket could never be read");

        assert_eq!(refused.reason, NotAttached::TicketUnavailable);
        assert_eq!(refused.shown.as_deref(), Some("holiday.heic"));
    }

    /// A host that cannot mint says so, and says which file it broke on. The
    /// file is fine; the operating system's random source is not.
    #[test]
    fn a_file_that_cannot_be_ticketed_is_refused_as_that() {
        let refused = attachment(
            Path::new("/Users/dev/Documents/notes.md"),
            ordinary(12),
            "text/markdown".to_string(),
            || {
                Err(NoTicket {
                    detail: "no randomness".to_string(),
                })
            },
        )
        .expect_err("a file with no ticket cannot be read later");

        assert_eq!(refused.reason, NotAttached::TicketUnavailable);
        assert_eq!(refused.shown.as_deref(), Some("notes.md"));
        assert_eq!(refused.detail.as_deref(), Some("no randomness"));
    }

    /// Cancelling is not a failure: the picker closes, nothing is attached, and
    /// the panel is told so by an empty answer. Reporting an error here would
    /// put a refusal on screen for somebody who simply changed their mind.
    #[test]
    fn cancelling_the_picker_attaches_nothing_and_refuses_nothing() {
        let files = Arc::new(FakeFiles::holding(10));

        assert_eq!(picking(Picked::Cancelled, files.clone()), Ok(Vec::new()));
        assert!(
            files.asked().is_empty(),
            "nothing was chosen, so nothing was looked up"
        );
    }

    /// No picker appeared. Unlike a cancellation there is nothing the person
    /// did, so this is the one outcome that owes them a sentence.
    #[test]
    fn a_picker_that_never_opened_is_reported_rather_than_read_as_a_cancellation() {
        let files = Arc::new(FakeFiles::holding(10));

        let refused = picking(
            Picked::Unavailable("the file picker did not open".to_string()),
            files.clone(),
        )
        .expect_err("nothing was chosen and nothing was asked");

        assert_eq!(refused.reason, NotAttached::PickerUnavailable);
        assert_eq!(
            refused.detail.as_deref(),
            Some("the file picker did not open")
        );
        assert!(files.asked().is_empty());
    }

    #[test]
    fn chosen_files_are_answered_in_the_order_the_picker_gave_them() {
        let first: PathBuf = ["/Users/dev/Documents", "first.md"].iter().collect();
        let second: PathBuf = ["/Users/dev/Documents", "second.md"].iter().collect();
        let files = Arc::new(FakeFiles::holding(64));

        let attached = picking(
            Picked::Files(vec![first.clone(), second.clone()]),
            files.clone(),
        )
        .expect("both files are attachable");

        assert_eq!(
            attached.iter().map(|file| &file.name).collect::<Vec<_>>(),
            vec!["first.md", "second.md"]
        );
        assert!(attached.iter().all(|file| file.size == 64));
        // Each chosen path was looked up, rather than one answer being reused.
        assert_eq!(files.asked(), vec![first, second]);
    }

    /// One ticket per chosen file, minted for the path the picker gave. A desk
    /// asked once for a selection of two would hand the same ticket to both,
    /// and reading one would spend the other.
    #[test]
    fn every_chosen_file_gets_its_own_ticket_for_its_own_path() {
        let first: PathBuf = ["/Users/dev/Documents", "first.md"].iter().collect();
        let second: PathBuf = ["/Users/dev/Documents", "second.md"].iter().collect();
        let tickets = Arc::new(FakeTickets::for_path("a-ticket", Path::new("/unused")));

        let attached = choose(
            Picked::Files(vec![first.clone(), second.clone()]),
            Arc::new(FakeFiles::holding(64)),
            "text/markdown",
            tickets.clone(),
        )
        .expect("both files are attachable");

        assert_eq!(tickets.paths(), vec![first, second]);
        assert!(
            attached.iter().all(|file| !file.ticket.is_empty()),
            "{attached:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_selection_with_one_unnameable_file_is_refused_whole() {
        let usable: PathBuf = ["/Users/dev/Documents", "notes.md"].iter().collect();
        let files = Arc::new(FakeFiles::holding(64));

        let refused = picking(Picked::Files(vec![usable, unnameable()]), files)
            .expect_err("a file that cannot be named refuses the selection");

        // Half an answer would be worse than none: the person chose two files
        // and would have been told nothing at all about the second.
        assert_eq!(refused.reason, NotAttached::PathNotText);
    }

    #[test]
    fn a_chosen_file_the_filesystem_refuses_is_reported_as_that() {
        let path: PathBuf = ["/Users/dev/Documents", "locked.pdf"].iter().collect();
        let files = Arc::new(FakeFiles::refusing(ErrorKind::PermissionDenied));

        let refused = picking(Picked::Files(vec![path.clone()]), files.clone())
            .expect_err("a file that cannot be read cannot be attached");

        assert_eq!(refused.reason, NotAttached::SizeUnreadable);
        assert_eq!(refused.shown.as_deref(), Some("locked.pdf"));
        assert_eq!(files.asked(), vec![path]);
    }

    /// The case the `stat` itself cannot settle, and the reason the deadline is
    /// here at all: the filesystem never comes back, so the host stops waiting
    /// and names the file it was waiting on.
    #[test]
    fn a_filesystem_that_never_answers_refuses_the_selection_at_the_deadline() {
        let path: PathBuf = ["/Volumes/gone", "holiday.heic"].iter().collect();

        let refused = tauri::async_runtime::block_on(chosen_attachments(
            &FakePicker(Picked::Files(vec![path])),
            Arc::new(FakeFiles::stalling(Duration::from_secs(30))),
            Arc::new(FakeTypes("image/heic")),
            Arc::new(FakeTickets::refusing(Redeemed::Unknown)),
            nothing_to_ready(),
            &Untold,
            Duration::from_millis(50),
            Duration::from_millis(50),
        ))
        .expect_err("a mount that is not answering has nothing to attach");

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

    /// An empty selection is the same fact as a cancellation — nothing was
    /// chosen — and must not become an error on the way through.
    #[test]
    fn a_picker_that_answered_with_nothing_attaches_nothing() {
        let files = Arc::new(FakeFiles::holding(10));

        assert_eq!(picking(Picked::Files(Vec::new()), files), Ok(Vec::new()));
    }

    /// The five fields are the contract with `src/host/window.ts`, and their
    /// names cross the seam in the shape the page reads them by. A Rust field
    /// renamed without the interface following would leave the panel reading
    /// `undefined` and routing every picked file as a document.
    #[test]
    fn a_chosen_file_crosses_the_seam_with_the_names_the_panel_reads() {
        let chosen = ChosenFile {
            path: "/Users/dev/Pictures/holiday.heic".to_string(),
            name: "holiday.heic".to_string(),
            size: 12,
            mime_type: "image/heic".to_string(),
            ticket: "a-ticket".to_string(),
        };

        assert_eq!(
            serde_json::to_string(&chosen).expect("a chosen file serializes"),
            r#"{"path":"/Users/dev/Pictures/holiday.heic","name":"holiday.heic","size":12,"mimeType":"image/heic","ticket":"a-ticket"}"#
        );
    }
}
