//! The operating system's own answer to "what kind of file is this?".
//!
//! The panel routes an attachment by its type and by nothing else: an image is
//! uploaded to the gateway and normalised there, anything else travels as a
//! path. A file that arrives by drop or paste gets that type from the browser,
//! which got it from the platform. A file that arrives through the host's
//! picker used to get no type at all, so the panel fell back to a hand-written
//! extension table — and every image format the platform knows and the table
//! does not (`.ico`, `.jpe`, `.svgz`, `.jp2`, `.xbm`, `.tga`, `.dib`) took a
//! *different route depending on how it was attached*. Growing the table only
//! moves the line; the platform's own database is the thing that is actually
//! current, and the host is the only part of Nessa that can ask it.
//!
//! So the host asks, and the panel puts the answer exactly where a browser's
//! `File.type` goes. The extension table stays where it belongs: as the
//! fallback for an empty answer, not as a second source of truth.
//!
//! ```text
//!   content_types() ──┬── macOS ──▶ NSURLContentTypeKey ─▶ UTType ─▶ MIME
//!                     ├── Linux ──▶ xdg-mime query filetype
//!                     └── else  ──▶ "" (and never a guess)
//! ```
//!
//! **Which platforms have a real implementation.** macOS and Linux do. Every
//! other target — Windows above all — answers with the empty string, always,
//! and the panel's extension fallback carries it. That is a deliberate gap
//! rather than an oversight: Windows's answer lives in the registry under
//! `HKEY_CLASSES_ROOT`, reading it needs a Windows API this host does not
//! otherwise touch, and there is no Windows build of Nessa to exercise it. An
//! empty answer is honest; a type invented from an extension inside the host
//! would be the extension table again, in the one place nobody would look for
//! it.
//!
//! Its own port rather than another question on [`super::files::ChosenFiles`],
//! and the reason is what is behind it. `ChosenFiles` is *the filesystem*: one
//! outside thing, asked twice, failing in one family of ways. This is the
//! platform's type database — a framework call on macOS and a subprocess on
//! Linux — with a different failure mode (it does not fail; it shrugs), a
//! different answer shape (a string that may be empty, never an error), and an
//! implementation that differs per target where `ChosenFiles` does not. Folding
//! it in would make the filesystem substitute carry three platforms' worth of
//! behaviour it has nothing to do with, and would make the "no such type"
//! answer indistinguishable from a file that could not be read.
//!
//! **What is not verified here, stated plainly.** Neither per-platform adapter
//! has a test and neither can have one that means anything: the macOS answer is
//! whatever Launch Services has been told about this machine's installed apps,
//! and the Linux answer is whatever shared-mime-info is installed, so a test
//! could only assert this machine's configuration back at itself. What *is*
//! tested is [`settled`] — the normalising every adapter's answer passes
//! through — and every way the panel reads the result. The framework call, the
//! `UTType` downcast, the subprocess and its exit status are exercised only by
//! running the app.

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod elsewhere;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod system;

pub use system::{content_types, ContentTypes};
