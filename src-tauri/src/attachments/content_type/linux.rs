//! Linux: the shared-mime-info answer, through `xdg-mime`.
//!
//! `xdg-mime query filetype` is the desktop's own answer for a path. It reads
//! the file's contents as well as its name — shared-mime-info is a database of
//! magic numbers, not a table of extensions — so a PNG named `.txt` is still a
//! PNG, and a format packaged after this build shipped is still known as soon
//! as the person's distribution updates its database.
//!
//! A subprocess rather than a library binding because a binding is not what is
//! already in the tree: `xdg-utils` is on every desktop that can put a file
//! picker on screen, the host already reaches for `xdg-open` the same way (see
//! `platform/linux`), and the alternative is a new C dependency for one string.
//! It is a process spawn, so it is slow in the way a process is slow — which is
//! why it happens inside the host's deadline along with the `stat`, and why the
//! adapter answers with the empty string rather than an error when the command
//! is missing, refuses, or is not on this machine's `PATH` at all.
//!
//! Untested, and it cannot usefully be otherwise: the answer is whatever
//! shared-mime-info this machine has installed, so a test could only assert
//! this machine's configuration back at itself.

use std::path::Path;
use std::process::Command;

use super::system::{settled, ContentTypes};

/// Injected by [`super::system::content_types`] on Linux.
pub struct SystemTypes;

impl ContentTypes for SystemTypes {
    fn of(&self, path: &Path) -> String {
        // The path is passed as an argument rather than interpolated into a
        // shell line: no shell is started, so a file named `; rm -rf ~` is an
        // argument and not a command.
        let Ok(answered) = Command::new("xdg-mime")
            .args(["query", "filetype"])
            .arg(path)
            .output()
        else {
            // No `xdg-mime` on this machine. The panel's extension fallback
            // carries it, exactly as it does on a platform with no adapter.
            return String::new();
        };
        if !answered.status.success() {
            return String::new();
        }

        settled(&String::from_utf8_lossy(&answered.stdout))
    }
}
