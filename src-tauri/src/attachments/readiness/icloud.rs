//! iCloud Drive, asked for a file it is keeping elsewhere.
//!
//! `startDownloadingUbiquitousItemAtURL:` begins the fetch and returns at once;
//! `NSURLUbiquitousItemDownloadingStatusKey` says where it has got to. Both are
//! iCloud's and only iCloud's, which is also how this handler knows a file is
//! its own: a path iCloud has no download status for is a path iCloud is not
//! keeping, and it is passed over.
//!
//! # What is verified
//!
//! The deadline, the landing, the failure to start and the handler declining a
//! file — all through [`ICloudService`], against a substitute. **The real
//! `NSFileManager` and `NSURL` behaviour is verified by nothing here.** No test
//! in this repository can create an iCloud placeholder, so the adapter at the
//! bottom of this file is exercised only by running the app against a real one.

use std::path::Path;
use std::time::{Duration, Instant};

use super::{MakeReadable, Readied, ReadyFuture};

/// Where iCloud says a file has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Here, and up to date.
    Current,
    /// On its way, or not started.
    Coming,
    /// Not a file iCloud is keeping.
    NotOurs,
}

/// iCloud itself, behind a port so every outcome is a line in a test —
/// including the two no real network can be asked for on demand: a download
/// still running when the deadline fires, and a service that will not take the
/// request at all.
pub trait ICloudService: Send + Sync {
    /// Ask for `path` to be fetched. Returns as soon as the request is made,
    /// never when it finishes.
    fn start(&self, path: &Path) -> Result<(), String>;

    /// Where `path` has got to.
    fn status(&self, path: &Path) -> Status;
}

/// The handler. Claims a file iCloud has a status for, and no other.
pub(super) struct ICloud;

impl MakeReadable for ICloud {
    fn mine(&self, path: &Path) -> bool {
        !matches!(system::ICloudFileManager.status(path), Status::NotOurs)
    }

    fn make_ready<'a>(
        &'a self,
        path: &'a Path,
        within: Duration,
        poll: Duration,
    ) -> ReadyFuture<'a> {
        Box::pin(fetch(path, &system::ICloudFileManager, within, poll))
    }
}

/// Wait for `path`'s bytes, for as long as `within` allows.
///
/// The loop lives here rather than in the adapter so that every outcome is a
/// line in a test. Asked once before the first sleep as well as after each one,
/// so a file that is already here costs no wait and no request.
pub(super) async fn fetch(
    path: &Path,
    icloud: &dyn ICloudService,
    within: Duration,
    poll: Duration,
) -> Readied {
    match icloud.status(path) {
        Status::Current => return Readied::Ready,
        // Claimed and then disowned between the two calls. Nothing to wait for.
        Status::NotOurs => return Readied::Refused("iCloud is not keeping this file".to_string()),
        Status::Coming => {}
    }
    if let Err(detail) = icloud.start(path) {
        return Readied::Refused(detail);
    }
    let started = Instant::now();
    loop {
        match icloud.status(path) {
            Status::Current => return Readied::Ready,
            Status::NotOurs => {
                return Readied::Refused("the download stopped being one iCloud knows".to_string())
            }
            Status::Coming => {}
        }
        if started.elapsed() >= within {
            // Nothing is held open across this, so there is nothing to release:
            // the request was made, iCloud owns it, and it carries on without
            // anybody waiting. Attaching again finds it landed.
            return Readied::StillComing;
        }
        tokio::time::sleep(poll).await;
    }
}

#[cfg(target_os = "macos")]
mod system {
    use std::path::Path;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{
        NSFileManager, NSString, NSURLUbiquitousItemDownloadingStatusCurrent,
        NSURLUbiquitousItemDownloadingStatusKey, NSURL,
    };

    use super::{ICloudService, Status};

    pub(super) struct ICloudFileManager;

    fn url_of(path: &Path) -> Option<Retained<NSURL>> {
        // A path that is not text cannot be made into an `NSString`, and is
        // about to be refused for that anyway.
        Some(NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)))
    }

    impl ICloudService for ICloudFileManager {
        fn start(&self, path: &Path) -> Result<(), String> {
            let Some(url) = url_of(path) else {
                return Err("the path is not text".to_string());
            };
            // The request returns as soon as it is made; the download is
            // watched by `status` below.
            NSFileManager::defaultManager()
                .startDownloadingUbiquitousItemAtURL_error(&url)
                .map_err(|error| error.localizedDescription().to_string())
        }

        fn status(&self, path: &Path) -> Status {
            let Some(url) = url_of(path) else {
                return Status::NotOurs;
            };
            let mut found: Option<Retained<AnyObject>> = None;
            // SAFETY: the key is documented to answer with an
            // `NSURLUbiquitousItemDownloadingStatus`, which is an `NSString`,
            // and the downcast below checks that rather than trusting it.
            let asked = unsafe {
                url.getResourceValue_forKey_error(
                    &mut found,
                    NSURLUbiquitousItemDownloadingStatusKey,
                )
            };
            // No answer at all is the ordinary case for a file iCloud is not
            // keeping — including another provider's placeholder, which is
            // dataless the same way and is not iCloud's to fetch.
            if asked.is_err() {
                return Status::NotOurs;
            }
            let Some(found) = found else {
                return Status::NotOurs;
            };
            let Ok(status) = found.downcast::<NSString>() else {
                return Status::NotOurs;
            };
            // `Current` is the only one that means the bytes are here and up to
            // date. `Downloaded` means a copy that is behind the one in the
            // cloud; asking for the current one is the honest thing to do when
            // somebody has just chosen the file.
            if unsafe { status.isEqualToString(NSURLUbiquitousItemDownloadingStatusCurrent) } {
                Status::Current
            } else {
                Status::Coming
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod system {
    use std::path::Path;

    use super::{ICloudService, Status};

    /// No iCloud here. Every file is somebody else's, so this handler claims
    /// none of them and the fallback answers.
    pub(super) struct ICloudFileManager;

    impl ICloudService for ICloudFileManager {
        fn start(&self, _path: &Path) -> Result<(), String> {
            Err("this platform has no iCloud to ask".to_string())
        }
        fn status(&self, _path: &Path) -> Status {
            Status::NotOurs
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// iCloud answering a written-down sequence, so every outcome is one line.
    struct Staged {
        answers: Mutex<Vec<Status>>,
        last: Status,
        start: Result<(), String>,
        asked: Mutex<Vec<PathBuf>>,
    }

    impl Staged {
        fn answering(answers: &[Status], last: Status) -> Self {
            Self {
                answers: Mutex::new(answers.iter().rev().copied().collect()),
                last,
                start: Ok(()),
                asked: Mutex::new(Vec::new()),
            }
        }
        fn refusing(detail: &str) -> Self {
            Self {
                answers: Mutex::new(Vec::new()),
                last: Status::Coming,
                start: Err(detail.to_string()),
                asked: Mutex::new(Vec::new()),
            }
        }
    }

    impl ICloudService for Staged {
        fn start(&self, path: &Path) -> Result<(), String> {
            self.asked
                .lock()
                .expect("the requests made")
                .push(path.to_path_buf());
            self.start.clone()
        }
        fn status(&self, _path: &Path) -> Status {
            self.answers
                .lock()
                .expect("the answers")
                .pop()
                .unwrap_or(self.last)
        }
    }

    fn wait_for(icloud: &Staged, within: Duration) -> Readied {
        tauri::async_runtime::block_on(fetch(
            Path::new("/Users/dev/iCloud/amica-document 2.pdf"),
            icloud,
            within,
            Duration::from_millis(1),
        ))
    }

    /// The case this exists for: a placeholder that lands while somebody waits
    /// a moment, after which it is an ordinary file.
    #[test]
    fn a_download_that_lands_inside_the_deadline_is_ready() {
        let icloud = Staged::answering(&[Status::Coming, Status::Coming], Status::Current);

        assert_eq!(wait_for(&icloud, Duration::from_secs(5)), Readied::Ready);
        assert_eq!(
            icloud.asked.lock().unwrap().as_slice(),
            [PathBuf::from("/Users/dev/iCloud/amica-document 2.pdf")]
        );
    }

    /// A file already here costs nothing at all: no request, no wait. The
    /// common path must not pay for this.
    #[test]
    fn a_file_already_here_is_ready_without_asking_for_anything() {
        let icloud = Staged::answering(&[], Status::Current);

        assert_eq!(wait_for(&icloud, Duration::from_secs(5)), Readied::Ready);
        assert!(icloud.asked.lock().unwrap().is_empty());
    }

    /// The bound, which is the whole reason this is not a plain await. A
    /// download on somebody else's network has no upper limit, and waiting for
    /// one without a deadline would leave `+` dead for the session.
    #[test]
    fn a_download_still_going_at_the_deadline_gives_up_rather_than_waiting() {
        let icloud = Staged::answering(&[], Status::Coming);
        let started = Instant::now();

        assert_eq!(
            wait_for(&icloud, Duration::from_millis(20)),
            Readied::StillComing
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "waited {:?}, which is not a deadline",
            started.elapsed()
        );
    }

    /// iCloud would not take the request, and says why in its own words rather
    /// than having them replaced by a guess.
    #[test]
    fn a_service_that_refuses_the_request_is_reported_with_its_own_words() {
        let icloud = Staged::refusing("The operation couldn’t be completed.");

        assert_eq!(
            wait_for(&icloud, Duration::from_secs(5)),
            Readied::Refused("The operation couldn’t be completed.".to_string())
        );
    }

    /// A file iCloud is not keeping is refused at once rather than waited on:
    /// there is nothing coming.
    #[test]
    fn a_file_icloud_does_not_keep_is_refused_without_a_request() {
        let icloud = Staged::answering(&[], Status::NotOurs);

        let answer = wait_for(&icloud, Duration::from_secs(5));

        assert!(matches!(answer, Readied::Refused(_)), "{answer:?}");
        assert!(icloud.asked.lock().unwrap().is_empty());
    }

    /// And one that stops being iCloud's partway through is refused rather
    /// than waited out, for the same reason.
    #[test]
    fn a_download_that_stops_being_icloud_s_is_refused_rather_than_waited_out() {
        let icloud = Staged::answering(&[Status::Coming], Status::NotOurs);

        let answer = wait_for(&icloud, Duration::from_secs(30));

        assert!(matches!(answer, Readied::Refused(_)), "{answer:?}");
    }
}
