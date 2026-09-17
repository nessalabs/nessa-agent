//! Asking the login keychain whether an item exists.

use std::process::{Command, Stdio};

/// Whether the login keychain holds a generic password for `service`.
///
/// This deliberately asks for the item's *attributes* and not its data:
/// `find-generic-password` without `-w` prints metadata and never the secret,
/// so it answers "is there a sign-in" without the app handling the token, and
/// without the keychain prompting for access to read one. Setup needs the
/// question answered, not the answer's contents.
///
/// Output is discarded and only the exit status is read. A missing binary, a
/// locked keychain or any other failure reads as absent, which is the safe
/// direction: the cost is offering to sign in again, where the alternative is
/// offering an agent that cannot run.
pub fn has_stored_credential(service: &str) -> bool {
    Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
