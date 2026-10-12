//! What saving a file the page hands over comes to, with the dialog and the
//! disk supplied: every outcome is written down without a window server.

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;

use serde::Serialize;

/// The most the page may hand over to save, in bytes: the largest file a
/// conversation holds, which is all the page ever saves. A bound on what one
/// request costs this process, checked before anybody is asked anything.
pub const LARGEST_SAVED_BYTES: usize = 64 * 1024 * 1024;

/// The longest name suggested in the dialog, in UTF-8 bytes: what the common
/// filesystems allow for one path component.
const LONGEST_NAME_BYTES: usize = 255;

/// Suggested when nothing of the page's name survives cleaning.
const FALLBACK_NAME: &str = "download";

/// The name the dialog suggests for `asked`: one file name and nothing else.
///
/// The page's name is the agent's — a model chose it — so it is held to being
/// a name for people, not a place. Anything before the last `/` or `\` is
/// dropped, so it names no folder; control characters, the characters Windows
/// and macOS refuse in a name, and the invisible ones that turn text around
/// (`U+202E` makes `gnp.exe` look like `exe.png`) are dropped; a leading or
/// trailing dot or space is dropped, so it is neither hidden nor `..`; and it
/// is cut to [`LONGEST_NAME_BYTES`] on a character boundary. Nothing left is
/// [`FALLBACK_NAME`]. It is only a suggestion: the person chooses in the
/// dialog where the file goes and what it is called.
pub fn suggested_name(asked: &str) -> String {
    let last = asked.rsplit(['/', '\\']).next().unwrap_or_default();
    let kept: String = last.chars().filter(|&c| !refused_in_name(c)).collect();
    let trimmed = kept.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut name = String::with_capacity(trimmed.len().min(LONGEST_NAME_BYTES));
    for c in trimmed.chars() {
        if name.len() + c.len_utf8() > LONGEST_NAME_BYTES {
            break;
        }
        name.push(c);
    }
    let name = name.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    if name.is_empty() {
        FALLBACK_NAME.to_string()
    } else {
        name.to_string()
    }
}

/// A character no suggested name carries.
fn refused_in_name(c: char) -> bool {
    c.is_control()
        || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')
        || matches!(
            c,
            '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// The page's name, from the header that carries it percent-encoded. `None`
/// when it is not percent-encoded UTF-8: a header carries ASCII only, so the
/// page encodes every name, and anything else is not this page's.
pub fn name_from_header(value: &[u8]) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut at = 0;
    while at < value.len() {
        match value[at] {
            b'%' => {
                let high = hex_digit(*value.get(at + 1)?)?;
                let low = hex_digit(*value.get(at + 2)?)?;
                bytes.push(high << 4 | low);
                at += 3;
            }
            byte if byte.is_ascii_graphic() => {
                bytes.push(byte);
                at += 1;
            }
            _ => return None,
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|digit| digit as u8)
}

/// What one turn at the save dialog produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chosen {
    /// Where the person chose to save it. The dialog has already asked them
    /// about replacing a file that is there.
    Path(PathBuf),
    /// They closed the dialog without choosing.
    Cancelled,
    /// No dialog came up, or what it answered is not a path on this machine;
    /// the host's own words for which.
    Unavailable(String),
}

/// Where "where should this file go?" is asked: the operating system's save
/// dialog, behind a port because it needs a window server and somebody to
/// click it.
pub trait SaveDestination: Send + Sync {
    /// Asks once, suggesting `name`, and waits as long as the person takes.
    fn choose(&self, name: String) -> Pin<Box<dyn Future<Output = Chosen> + Send>>;
}

/// What became of a save the page asked for, when nothing went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Saved {
    /// The bytes are in the file the person chose.
    Saved,
    /// The person closed the dialog; nothing was written. Not a failure.
    Cancelled,
}

/// Why nothing was saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotSavedReason {
    /// More than [`LARGEST_SAVED_BYTES`]; refused before anybody was asked.
    TooLarge,
    /// The request was not bytes with a percent-encoded name.
    Malformed,
    /// No save dialog came up.
    NoDialog,
    /// The person chose where, and the bytes could not be written there.
    NotWritten,
}

/// Nothing was saved, and why. `detail` is for the diagnostics, not the screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileNotSaved {
    pub reason: NotSavedReason,
    pub detail: String,
}

impl FileNotSaved {
    pub fn too_large(size: usize, most: usize) -> Self {
        Self {
            reason: NotSavedReason::TooLarge,
            detail: format!("{size} bytes is more than the {most} a save takes"),
        }
    }

    pub fn malformed(detail: &str) -> Self {
        Self {
            reason: NotSavedReason::Malformed,
            detail: detail.to_string(),
        }
    }
}

/// One save, from the page's bytes and name to the file on disk.
///
/// The order is the rule: the size is refused before the dialog comes up, so
/// nobody chooses a place for a file that was never going to be written; the
/// name is cleaned before it is suggested; the bytes are written only where
/// the person chose, and only once they have.
pub async fn saved<W, F>(
    destination: &dyn SaveDestination,
    name: &str,
    bytes: Vec<u8>,
    most: usize,
    write: W,
) -> Result<Saved, FileNotSaved>
where
    W: FnOnce(PathBuf, Vec<u8>) -> F,
    F: Future<Output = io::Result<()>>,
{
    if bytes.len() > most {
        return Err(FileNotSaved::too_large(bytes.len(), most));
    }
    match destination.choose(suggested_name(name)).await {
        Chosen::Cancelled => Ok(Saved::Cancelled),
        Chosen::Unavailable(detail) => Err(FileNotSaved {
            reason: NotSavedReason::NoDialog,
            detail,
        }),
        Chosen::Path(path) => match write(path, bytes).await {
            Ok(()) => Ok(Saved::Saved),
            Err(error) => Err(FileNotSaved {
                reason: NotSavedReason::NotWritten,
                detail: error.to_string(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A dialog that answers what it was given, and remembers what it suggested.
    struct Answering {
        answer: Chosen,
        suggested: Mutex<Vec<String>>,
    }

    impl Answering {
        fn with(answer: Chosen) -> Self {
            Self {
                answer,
                suggested: Mutex::new(Vec::new()),
            }
        }
        fn suggested(&self) -> Vec<String> {
            self.suggested.lock().unwrap().clone()
        }
    }

    impl SaveDestination for Answering {
        fn choose(&self, name: String) -> Pin<Box<dyn Future<Output = Chosen> + Send>> {
            self.suggested.lock().unwrap().push(name);
            let answer = self.answer.clone();
            Box::pin(async move { answer })
        }
    }

    async fn to_disk(path: PathBuf, bytes: Vec<u8>) -> io::Result<()> {
        std::fs::write(path, bytes)
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        // Every future here is ready the first time it is polled.
        use std::task::{Context, Poll, Waker};
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("a substitute dialog or disk waited"),
        }
    }

    #[test]
    fn writes_the_bytes_where_the_person_chose_under_the_cleaned_name() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("chosen.png");
        let dialog = Answering::with(Chosen::Path(path.clone()));
        let outcome = block_on(saved(
            &dialog,
            "../../.ssh/chart.png",
            b"\x89PNG bytes".to_vec(),
            LARGEST_SAVED_BYTES,
            to_disk,
        ));
        assert_eq!(outcome, Ok(Saved::Saved));
        assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG bytes");
        assert_eq!(dialog.suggested(), ["chart.png"]);
    }

    #[test]
    fn writes_nothing_when_the_person_cancels() {
        let folder = tempfile::tempdir().unwrap();
        let dialog = Answering::with(Chosen::Cancelled);
        let outcome = block_on(saved(&dialog, "a.txt", b"a".to_vec(), 10, to_disk));
        assert_eq!(outcome, Ok(Saved::Cancelled));
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    }

    #[test]
    fn refuses_too_much_before_any_dialog_comes_up() {
        let dialog = Answering::with(Chosen::Cancelled);
        let outcome = block_on(saved(&dialog, "a.txt", vec![0; 11], 10, to_disk));
        assert_eq!(outcome.unwrap_err().reason, NotSavedReason::TooLarge);
        assert!(dialog.suggested().is_empty());
    }

    #[test]
    fn says_when_no_dialog_came_up_and_when_the_disk_refused() {
        let dialog = Answering::with(Chosen::Unavailable("no window".into()));
        let outcome = block_on(saved(&dialog, "a.txt", b"a".to_vec(), 10, to_disk));
        assert_eq!(outcome.unwrap_err().reason, NotSavedReason::NoDialog);

        let folder = tempfile::tempdir().unwrap();
        // A folder that does not exist cannot hold the file.
        let nowhere = folder.path().join("missing").join("a.txt");
        let dialog = Answering::with(Chosen::Path(nowhere.clone()));
        let outcome = block_on(saved(&dialog, "a.txt", b"a".to_vec(), 10, to_disk));
        assert_eq!(outcome.unwrap_err().reason, NotSavedReason::NotWritten);
        assert!(!nowhere.exists());
    }

    #[test]
    fn suggests_one_plain_file_name() {
        for (asked, suggested) in [
            ("report.pdf", "report.pdf"),
            ("/etc/passwd", "passwd"),
            ("C:\\Users\\me\\notes.txt", "notes.txt"),
            ("..", "download"),
            ("", "download"),
            ("   .hidden  ", "hidden"),
            ("trail. . ", "trail"),
            ("a<b>c:d\"e|f?g*h.txt", "abcdefgh.txt"),
            ("line\nbreak\u{7}.txt", "linebreak.txt"),
            ("photo\u{202E}gnp.exe", "photognp.exe"),
            ("résumé.pdf", "résumé.pdf"),
        ] {
            assert_eq!(suggested_name(asked), suggested, "for {asked:?}");
        }
    }

    #[test]
    fn cuts_a_long_name_on_a_character_boundary() {
        let long = "é".repeat(200);
        let name = suggested_name(&long);
        assert!(name.len() <= LONGEST_NAME_BYTES);
        assert_eq!(name, "é".repeat(127));
    }

    #[test]
    fn reads_the_name_back_from_its_header() {
        assert_eq!(
            name_from_header(b"r%C3%A9sum%C3%A9%201%2F2.pdf").as_deref(),
            Some("résumé 1/2.pdf")
        );
        assert_eq!(name_from_header(b"plain.txt").as_deref(), Some("plain.txt"));
        // Not percent-encoded UTF-8: not this page's.
        assert_eq!(name_from_header(b"%C3"), None);
        assert_eq!(name_from_header(b"%zz"), None);
        assert_eq!(name_from_header(b"two words"), None);
        assert_eq!(name_from_header(b"cut%2"), None);
    }

    #[test]
    fn says_its_outcomes_as_the_page_reads_them() {
        assert_eq!(serde_json::to_value(Saved::Saved).unwrap(), "saved");
        assert_eq!(serde_json::to_value(Saved::Cancelled).unwrap(), "cancelled");
        assert_eq!(
            serde_json::to_value(FileNotSaved {
                reason: NotSavedReason::NotWritten,
                detail: "read-only".into()
            })
            .unwrap(),
            serde_json::json!({ "reason": "not-written", "detail": "read-only" })
        );
    }
}
