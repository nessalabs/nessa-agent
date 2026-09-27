//! The port, the one shape every platform's answer is made to fit, and the
//! compile-time choice of which platform answers.

use std::path::Path;
use std::sync::Arc;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use super::elsewhere::SystemTypes;
#[cfg(target_os = "linux")]
use super::linux::SystemTypes;
#[cfg(target_os = "macos")]
use super::macos::SystemTypes;

/// Where "what does this machine think this file is?" is asked.
///
/// One method, answering with a string, and never with an error. A platform
/// that has no opinion about a path says so with an empty string, which is a
/// fact the panel acts on — it falls back to the extension it can see — rather
/// than a failure it would have to report. There is nothing a person could do
/// about "the type database did not answer", so there is nothing to tell them.
pub trait ContentTypes: Send + Sync {
    /// The operating system's own content type for `path`.
    ///
    /// Lowercase, with no parameters, in the shape a browser's `File.type`
    /// arrives in — because that is exactly where the panel puts it. Empty when
    /// this platform has no answer, and empty rather than guessed: an
    /// extension is the panel's fallback and must not also be the host's.
    ///
    /// Blocking, and called from inside [`super::super::files::answered_within`]
    /// for that reason: the Linux answer is a subprocess and the macOS answer
    /// touches the filesystem, so either can hang on a stalled mount exactly as
    /// a `stat` can.
    fn of(&self, path: &Path) -> String;
}

/// One platform's raw answer, in the shape the panel is promised.
///
/// Trimmed, cut at the first `;` so a `text/plain; charset=utf-8` becomes the
/// type without the parameter, lowercased, and — the part that is a rule rather
/// than tidying — discarded entirely unless it looks like a type at all. A
/// platform tool that answers with a sentence, a warning, or an empty line must
/// produce the same empty string as a platform with no answer, because the
/// panel branches on `startsWith("image/")` and a stray sentence is not
/// something to branch on.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn settled(answer: &str) -> String {
    let named = answer
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let (kind, subtype) = match named.split_once('/') {
        Some(split) => split,
        None => return String::new(),
    };
    // Exactly one slash, both halves present, and no whitespace anywhere: that
    // is the whole of what a media type looks like, and anything else is a tool
    // saying something other than a type.
    if kind.is_empty()
        || subtype.is_empty()
        || subtype.contains('/')
        || named.contains(char::is_whitespace)
    {
        return String::new();
    }
    named
}

/// The type database this build asks. Called from composition.
///
/// Compile-time injection, the same shape as [`crate::platform::current`]: the
/// binary is built with exactly one adapter, so there is no runtime switch and
/// no `cfg` anywhere the answer is used.
pub fn content_types() -> Arc<dyn ContentTypes> {
    Arc::new(SystemTypes)
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    /// The ordinary answer passes through as itself.
    #[test]
    fn a_plain_type_is_carried_through() {
        assert_eq!(settled("image/png"), "image/png");
    }

    /// The two shapes a platform tool actually answers in: a trailing newline
    /// from a subprocess, and a parameter the panel has no use for.
    #[test]
    fn whitespace_and_parameters_are_cut_away() {
        assert_eq!(settled("image/svg+xml\n"), "image/svg+xml");
        assert_eq!(settled("text/plain; charset=utf-8"), "text/plain");
        assert_eq!(settled("  application/pdf  "), "application/pdf");
    }

    /// The panel compares against lowercase names, so the host hands it
    /// lowercase names — a platform that shouts is not a platform with a
    /// different type.
    #[test]
    fn an_answer_is_lowercased() {
        assert_eq!(settled("IMAGE/JPEG"), "image/jpeg");
    }

    /// The rule rather than the tidying. Anything that is not a type is no
    /// answer at all, so a tool that prints a complaint cannot make the panel
    /// branch on it.
    #[test]
    fn anything_that_is_not_a_type_is_no_answer() {
        for answer in [
            "",
            "\n",
            "unknown",
            "xdg-mime: command not found",
            "/png",
            "image/",
            "image/png is what it is",
            "image/png/and-more",
            ";charset=utf-8",
        ] {
            assert_eq!(settled(answer), "", "{answer:?}");
        }
    }

    /// The case the whole thing exists for: a format the panel's extension
    /// table has never heard of still arrives as an image, because the platform
    /// knows it and the host asked.
    #[test]
    fn a_type_the_panels_table_lacks_still_arrives_as_an_image() {
        for answer in ["image/vnd.microsoft.icon", "image/jp2", "image/x-xbitmap"] {
            assert!(settled(answer).starts_with("image/"), "{answer}");
        }
    }
}
