//! Everywhere else — Windows above all: no answer, and no guess.
//!
//! This is the adapter the module header's last paragraph describes, written
//! out so the gap is a thing in the source rather than an absence. Windows does
//! have an answer, in the registry under `HKEY_CLASSES_ROOT`; reading it needs
//! a Windows API this host does not otherwise touch, and there is no Windows
//! build of Nessa to exercise it against. Until there is, this says nothing,
//! which is true, and the panel's extension fallback carries the file.
//!
//! What it must never do is invent a type from the extension. The panel already
//! has that table; a second copy of it here, inside the thing whose whole job
//! is to be the platform's answer, would make the two disagree silently and
//! would be the hardest kind of bug to find.

use std::path::Path;

use super::system::ContentTypes;

/// Injected by [`super::system::content_types`] off macOS and Linux.
pub struct SystemTypes;

impl ContentTypes for SystemTypes {
    fn of(&self, _path: &Path) -> String {
        String::new()
    }
}
