//! macOS: the path's Uniform Type Identifier, as its preferred MIME type.
//!
//! `NSURLContentTypeKey` is the filesystem's own answer for a path — Launch
//! Services consults the file, its extension, and anything the volume records
//! about it, which is more than an extension and is current in a way a table
//! compiled into Nessa can never be. It answers with a `UTType`, and a
//! `UTType`'s `preferredMIMEType` is the same string a browser would have put
//! in `File.type` for the same file. That is the whole point: a file dropped
//! into the panel and the same file picked through the host now arrive with the
//! same type, so they take the same route.
//!
//! A type macOS knows and has no MIME name for — a private UTI, a bundle —
//! answers `nil`, and this reports that as the empty string. So does a path
//! that is not text, a URL the framework would not describe, and a value that
//! is not a `UTType` after all. Never a guess: see the module header above.
//!
//! Untested, and it cannot usefully be otherwise. The answer is whatever Launch
//! Services has been told about this particular machine's installed
//! applications, so a test could only assert this machine's configuration back
//! at itself.

use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSString, NSURLContentTypeKey, NSURL};
use objc2_uniform_type_identifiers::UTType;

use super::system::{settled, ContentTypes};

/// Injected by [`super::system::content_types`] on macOS.
pub struct SystemTypes;

impl ContentTypes for SystemTypes {
    fn of(&self, path: &Path) -> String {
        // A path that is not text cannot be made into an `NSString`, and it is
        // about to be refused for that anyway — so there is nothing to ask.
        let Some(text) = path.to_str() else {
            return String::new();
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(text));

        let mut found: Option<Retained<AnyObject>> = None;
        // SAFETY: `NSURLContentTypeKey` is documented to answer with a
        // `UTType`, and the downcast below checks that rather than trusting it.
        // The error is discarded on purpose: "this volume does not record a
        // type" is not a failure the person can act on, and the empty string
        // already says it.
        let asked = unsafe { url.getResourceValue_forKey_error(&mut found, NSURLContentTypeKey) };
        if asked.is_err() {
            return String::new();
        }
        let Some(found) = found else {
            return String::new();
        };
        let Ok(kind) = found.downcast::<UTType>() else {
            return String::new();
        };
        // A type macOS knows and has no MIME name for: a private UTI, or a
        // bundle. Nothing to carry, and nothing to invent.
        let Some(name) = kind.preferredMIMEType() else {
            return String::new();
        };

        settled(&name.to_string())
    }
}
