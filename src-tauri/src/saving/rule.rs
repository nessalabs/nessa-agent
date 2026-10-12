//! The words of a save — what the page is told, the name the dialog suggests,
//! and the dialog as a port — with no window server and no disk.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use serde::Serialize;

/// The most the page may save as one file, in bytes: the largest file a
/// conversation holds, which is all the page ever saves.
pub const LARGEST_SAVED_BYTES: u64 = 64 * 1024 * 1024;

/// The most one part of a save may carry, in bytes. A bound on what one call
/// costs: under the webview's `postMessage` fallback a raw body travels as a
/// JSON array of one number per byte, and this keeps that array bounded too.
pub const LARGEST_PART_BYTES: usize = 1024 * 1024;

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

/// What beginning a save came to, when nothing went wrong.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Begun {
    /// The person chose where; the parts go to this save by its token.
    Open { token: String },
    /// The person closed the dialog; nothing was opened. Not a failure.
    Cancelled,
}

/// Why nothing was saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotSavedReason {
    /// A part past [`LARGEST_PART_BYTES`], or a file past
    /// [`LARGEST_SAVED_BYTES`].
    TooLarge,
    /// The request was not what a save sends.
    Malformed,
    /// No save dialog came up.
    NoDialog,
    /// No save is open under that token: never begun, finished, given up, or
    /// left idle until the host let it go.
    NotOpen,
    /// The bytes could not be written where the person chose.
    NotWritten,
}

/// Nothing was saved, and why. `detail` is for the diagnostics, not the screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileNotSaved {
    pub reason: NotSavedReason,
    pub detail: String,
}

impl FileNotSaved {
    pub fn new(reason: NotSavedReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn says_its_outcomes_as_the_page_reads_them() {
        assert_eq!(
            serde_json::to_value(Begun::Open { token: "t".into() }).unwrap(),
            serde_json::json!({ "state": "open", "token": "t" })
        );
        assert_eq!(
            serde_json::to_value(Begun::Cancelled).unwrap(),
            serde_json::json!({ "state": "cancelled" })
        );
        assert_eq!(
            serde_json::to_value(FileNotSaved::new(NotSavedReason::NotOpen, "gone")).unwrap(),
            serde_json::json!({ "reason": "not-open", "detail": "gone" })
        );
    }
}
