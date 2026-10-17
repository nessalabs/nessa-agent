//! What a review notice is, and what the system lets the host do with one.

use std::fmt::Write as _;

/// The longest id part the host accepts: a conversation, execution or
/// permission id. The gateway's own ids are far shorter.
pub(crate) const MAX_ID_BYTES: usize = 256;
/// The longest title a notice shows, in characters. A tool name past this is
/// cut, not refused: the notice is still worth showing.
pub(crate) const MAX_TITLE_CHARS: usize = 120;
/// The longest body a notice shows, in characters.
pub(crate) const MAX_BODY_CHARS: usize = 240;

/// Which waiting review a notice is for. Two notices for the same review are
/// the same notice.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NoticeId {
    conversation: String,
    identifier: String,
}

/// Why the window's notice was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeRefusal {
    /// An id part was empty, too long, or held a control character.
    MalformedId,
}

impl NoticeId {
    /// The notice for one review, named the way the gateway names it.
    pub(crate) fn new(
        conversation: &str,
        execution: &str,
        permission: &str,
    ) -> Result<Self, NoticeRefusal> {
        for part in [conversation, execution, permission] {
            if part.is_empty() || part.len() > MAX_ID_BYTES || part.chars().any(char::is_control) {
                return Err(NoticeRefusal::MalformedId);
            }
        }
        // Hex keeps the system's identifier free of whatever the ids hold,
        // and the lengths keep `a`+`bc` apart from `ab`+`c`.
        let mut identifier = String::from("nessa.review.");
        for part in [conversation, execution, permission] {
            let _ = write!(identifier, "{}.", part.len());
            for byte in part.bytes() {
                let _ = write!(identifier, "{byte:02x}");
            }
            identifier.push('.');
        }
        identifier.pop();
        Ok(Self {
            conversation: conversation.to_owned(),
            identifier,
        })
    }

    /// The conversation the review waits in: where opening the notice goes.
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    pub(crate) fn conversation(&self) -> &str {
        &self.conversation
    }

    /// The notice's name to the system, the same for the same review. Linux's
    /// service names notices itself.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn identifier(&self) -> &str {
        &self.identifier
    }
}

/// One notice: which review, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewNotice {
    pub(crate) id: NoticeId,
    pub(crate) title: String,
    pub(crate) body: String,
}

impl ReviewNotice {
    /// A notice whose text is cut to what a notification shows.
    pub(crate) fn new(id: NoticeId, title: &str, body: &str) -> Self {
        Self {
            id,
            title: clip(title, MAX_TITLE_CHARS),
            body: clip(body, MAX_BODY_CHARS),
        }
    }
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        None => text.to_owned(),
        Some((end, _)) => {
            let mut cut = text[..end].trim_end().to_owned();
            cut.push('…');
            cut
        }
    }
}

/// Whether the system lets this app show notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeAccess {
    /// This host has no notices the app can post and take back.
    Unsupported,
    /// The person has not been asked yet. Only macOS asks.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NotAsked,
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    Allowed,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Denied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_review_is_the_same_notice() {
        let one = NoticeId::new("c1", "e1", "p1").unwrap();
        let again = NoticeId::new("c1", "e1", "p1").unwrap();
        assert_eq!(one, again);
        assert_eq!(one.identifier(), again.identifier());
        assert_eq!(one.conversation(), "c1");
    }

    #[test]
    fn parts_cannot_run_into_each_other() {
        let left = NoticeId::new("ab", "c", "d").unwrap();
        let right = NoticeId::new("a", "bc", "d").unwrap();
        assert_ne!(left.identifier(), right.identifier());
    }

    #[test]
    fn the_identifier_holds_only_plain_characters() {
        let id = NoticeId::new("conv/ü", "exec \"x\"", "perm").unwrap();
        assert!(id
            .identifier()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.'));
    }

    #[test]
    fn malformed_parts_are_refused() {
        assert_eq!(NoticeId::new("", "e", "p"), Err(NoticeRefusal::MalformedId));
        assert_eq!(
            NoticeId::new("c", "e\0", "p"),
            Err(NoticeRefusal::MalformedId)
        );
        let long = "x".repeat(MAX_ID_BYTES + 1);
        assert_eq!(
            NoticeId::new("c", "e", &long),
            Err(NoticeRefusal::MalformedId)
        );
        assert!(NoticeId::new("c", "e", &"x".repeat(MAX_ID_BYTES)).is_ok());
    }

    #[test]
    fn long_text_is_cut_on_a_character() {
        let id = NoticeId::new("c", "e", "p").unwrap();
        let notice = ReviewNotice::new(id, &"é".repeat(500), "  short  ");
        assert_eq!(notice.title.chars().count(), MAX_TITLE_CHARS + 1);
        assert!(notice.title.ends_with('…'));
        assert_eq!(notice.body, "short");
    }
}
