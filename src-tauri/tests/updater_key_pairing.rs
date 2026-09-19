//! Whether the public key we ship is the other half of the key we sign with.
//!
//! Everything else about the updater can be right and this one fact wrong, and
//! the result is unrecoverable: a build that trusts the wrong public key rejects
//! every signature the release process will ever produce, forever, and the only
//! way back is asking every person who installed it to download Nessa by hand.
//! No later release can fix it, because no later release can be installed.
//!
//! `scripts/desktop/config.test.mjs` already checks that the configured key
//! *looks* like a minisign public key and is not a secret key. That is a shape
//! check. A perfectly shaped public key from a keypair nobody has the other half
//! of passes it. This asks the only question that settles it: sign something
//! with the release private key, and see whether the key in `tauri.conf.json`
//! says yes.
//!
//! ```text
//!   tauri.conf.json ──pubkey──┐
//!                             ▼
//!   fixture ──▶ ReleaseSigner ──Signing──▶ pairing ──▶ Pairing
//!                 │        │                            │  │  │
//!         TauriSigner   fixed               Paired ◀────┘  │  └──▶ Unverified
//!         (pnpm tauri   (embedded                          ▼
//!          signer sign)  signatures)                  Mismatched
//! ```
//!
//! The signer is a port because signing reaches a private key on a disk we do
//! not control and a subprocess we do not own. Its substitute is a pair of
//! signatures embedded below, made once by a throwaway keypair, which is what
//! lets the mismatch case — the one that matters and the one nobody can stage —
//! be a permanent offline test rather than a thing we hope we would notice.
//!
//! [`pairing`] is pure, and it is the whole rule. It verifies exactly the way
//! `tauri-plugin-updater` does at runtime — base64-decode the configured key,
//! base64-decode the signature block, `minisign_verify::PublicKey::verify` with
//! legacy signatures allowed — using the same crate and the same version the
//! plugin itself pulls in, so a pass here is the real verifier saying yes.
//!
//! # Running it
//!
//! Point it at the release private key and run the test:
//!
//! ```text
//! TAURI_SIGNING_PRIVATE_KEY_PATH=~/.nessa-signing/updater.key \
//! TAURI_SIGNING_PRIVATE_KEY_PASSWORD= \
//!   cargo test -p nessa-app --test updater_key_pairing
//! ```
//!
//! Without those variables there is no key on the machine, and an ordinary
//! `cargo test` prints a loud skip naming what went unverified. A skip is not a
//! pass and must never read like one, so anywhere the key *is* available —
//! above all the release workflow, which holds it as
//! `TAURI_SIGNING_PRIVATE_KEY` — set `NESSA_REQUIRE_UPDATER_KEY_PAIRING=1` and
//! the absence of the key becomes a failure too. A *mismatch* fails everywhere,
//! with or without that variable: there is no configuration in which the wrong
//! key is tolerated.

use std::env::{self, VarError};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use minisign_verify::{PublicKey, Signature};

/// Set where the release private key is present and a skip would be a lie.
const REQUIRE_PAIRING: &str = "NESSA_REQUIRE_UPDATER_KEY_PAIRING";

/// The bytes signed. Content is irrelevant — only who can sign it matters.
const FIXTURE: &[u8] = b"nessa updater key pairing fixture\n";

/// A throwaway public key, and a signature it made over [`FIXTURE`].
///
/// Generated once with `tauri signer generate` into a temporary directory; the
/// private half was never written down and no longer exists. It stands in for
/// "some other keypair" — the shape of the accident this test exists to catch,
/// where `tauri.conf.json` carries a real, well-formed public key belonging to
/// a private key the release process does not have.
const OTHER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEZERDNFRjIxMzgwMkQ5RjUKUldUMTJRSTRJZS9UL2ZtYjc0a1ZESmxhci9CL0l1dzV1REhrc3N4QXBHbnY0OGtnU29jcVVDZUsK";
const OTHER_SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUMTJRSTRJZS9UL1J3WDJCWWh6NXFtL2RzS1FqQVZ1Z2dGeWpQTXc2ZEk5dXdHSllVdDlPRXRnY0N1aUNRQWdhR0xlSWpiZ3ltcEJhS0FjM1NjRG5FZFp6RFFISEJTdWdBPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg5Njk4NjU4CWZpbGU6cGF5bG9hZC5iaW4KUWVxMzZhaG1uODlyNkJrUHhWbGxYRitTU2twc0pCNGs2ay8yMVRKTmZOUGNmWlhRQm9ZQU4vT0ZDeWgzSWJMSlZ3ZjhyTEwrMDVyZzJoQnFHTjU1RFE9PQo=";

/// What asking the release private key for a signature produced.
///
/// [`Signing::Absent`] is a first-class answer rather than an error string
/// because it is the ordinary outcome on a contributor's machine and the one
/// outcome that must never be confused with success.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Signing {
    /// A signature block, base64-encoded exactly as Tauri writes `.sig` files
    /// and exactly as it appears in a release manifest's `signature` field.
    Signed(String),
    /// No release private key is reachable from here.
    Absent(String),
    /// A key was named but signing did not produce a usable signature.
    Refused(String),
}

/// Where a signature by the release private key comes from.
///
/// A port because the key sits on a disk outside this repository and signing it
/// means running the Tauri CLI: neither is reachable from a test that has to
/// pass on a machine that has never held the key.
trait ReleaseSigner {
    /// Signs [`FIXTURE`] once.
    fn sign(&self) -> Signing;
}

/// The real signer: `pnpm tauri signer sign`, the same command a release runs.
///
/// Deliberately the CLI rather than a Rust signing crate. The question is
/// whether the shipped public key matches what *the release process* produces,
/// so the release process's own tool has to be the one that produces it.
struct TauriSigner {
    /// Repository root; the CLI resolves `tauri` from the workspace's modules.
    root: PathBuf,
    /// A scratch directory the `.sig` file may be written into.
    scratch: PathBuf,
}

impl ReleaseSigner for TauriSigner {
    fn sign(&self) -> Signing {
        let (flag, key) =
            match (
                optional_var("TAURI_SIGNING_PRIVATE_KEY_PATH"),
                optional_var("TAURI_SIGNING_PRIVATE_KEY"),
            ) {
                (Some(path), _) => ("--private-key-path", path),
                (None, Some(key)) => ("--private-key", key),
                (None, None) => return Signing::Absent(
                    "neither TAURI_SIGNING_PRIVATE_KEY_PATH nor TAURI_SIGNING_PRIVATE_KEY is set"
                        .to_string(),
                ),
            };

        let payload = self.scratch.join("updater-key-pairing.bin");
        let produced = payload.with_extension("bin.sig");
        // A leftover signature from an earlier run would otherwise be read back
        // as this run's answer, which is the one way this test could pass while
        // signing failed.
        let _ = fs::remove_file(&produced);
        if let Err(error) =
            fs::create_dir_all(&self.scratch).and_then(|()| fs::write(&payload, FIXTURE))
        {
            return Signing::Refused(format!("could not stage the fixture: {error}"));
        }

        let pnpm = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
        let run = Command::new(pnpm)
            .current_dir(&self.root)
            .args(["exec", "tauri", "signer", "sign", flag, &key])
            // The release key has an empty password. Passing it explicitly stops
            // the CLI stopping at an interactive prompt nobody is there to answer.
            .args([
                "--password",
                &optional_var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default(),
            ])
            .arg(&payload)
            .output();

        let run = match run {
            Ok(run) => run,
            Err(error) => {
                return Signing::Refused(format!("could not run `{pnpm} exec tauri`: {error}"))
            }
        };
        if !run.status.success() {
            return Signing::Refused(format!(
                "`tauri signer sign` failed: {}",
                String::from_utf8_lossy(&run.stderr).trim()
            ));
        }

        match fs::read_to_string(&produced) {
            Ok(signature) => Signing::Signed(signature),
            Err(error) => Signing::Refused(format!(
                "`tauri signer sign` reported success but wrote no signature to {}: {error}",
                produced.display()
            )),
        }
    }
}

/// A signer holding one signature already, for the cases nobody can stage.
struct FixedSigning(Signing);

impl ReleaseSigner for FixedSigning {
    fn sign(&self) -> Signing {
        self.0.clone()
    }
}

/// Whether the configured public key and the signing key are two halves of one
/// keypair.
///
/// The three ways of not being [`Pairing::Paired`] are kept apart on purpose,
/// because only one of them is survivable. [`Pairing::Mismatched`] means the
/// answer was no. [`Pairing::Unverified`] means nobody asked, because there was
/// no key here to ask with — the ordinary state of a contributor's machine.
/// [`Pairing::Unusable`] means somebody did ask, with a key they named, and the
/// signer failed: a wrong path, an unavailable command, a bad password. That is
/// a broken signing setup, not an absent one, and letting it share a variant
/// with "nobody asked" is how a release goes out on an unchecked key.
#[derive(Debug, PartialEq, Eq)]
enum Pairing {
    /// The configured key verified a signature by the signing key.
    Paired,
    /// The signature exists and the configured key rejects it. Every shipped
    /// build would reject every release the same way.
    Mismatched(String),
    /// The question was not asked, because nothing here could ask it. Never
    /// evidence of anything.
    Unverified(String),
    /// A signing key was named and did not produce a signature. The question
    /// was asked and the machinery for answering it is broken.
    Unusable(String),
}

/// The rule, in the plugin's own terms.
///
/// `tauri_plugin_updater`'s `verify_signature` base64-decodes the configured
/// `pubkey`, base64-decodes the manifest's `signature`, and calls
/// `PublicKey::verify(data, &signature, true)`. This is that, on the same crate
/// version, so a `Paired` here is the runtime verifier answering yes.
fn pairing(configured_pubkey: &str, payload: &[u8], signing: &Signing) -> Pairing {
    let signature = match signing {
        Signing::Signed(signature) => signature,
        Signing::Absent(why) => {
            return Pairing::Unverified(format!("no release private key to sign with: {why}"))
        }
        Signing::Refused(why) => {
            return Pairing::Unusable(format!(
                "the release key did not produce a signature: {why}"
            ))
        }
    };

    let key = match decode_as_the_runtime_does(configured_pubkey)
        .as_deref()
        .map(PublicKey::decode)
    {
        Some(Ok(key)) => key,
        Some(Err(error)) => {
            return Pairing::Mismatched(format!(
                "the configured pubkey is not a minisign public key: {error}"
            ))
        }
        None => return Pairing::Mismatched("the configured pubkey is not base64".to_string()),
    };
    let signature = match decode_signature_file(signature)
        .as_deref()
        .map(Signature::decode)
    {
        Some(Ok(signature)) => signature,
        Some(Err(error)) => {
            return Pairing::Mismatched(format!(
                "the signature is not a minisign signature: {error}"
            ))
        }
        None => return Pairing::Mismatched("the signature is not base64".to_string()),
    };

    // `true` allows legacy (non-prehashed) signatures, which is what the Tauri
    // CLI produces and what the plugin accepts. Verifying more strictly here
    // than the plugin does would fail builds that ship perfectly well.
    match key.verify(payload, &signature, true) {
        Ok(()) => Pairing::Paired,
        Err(error) => Pairing::Mismatched(error.to_string()),
    }
}

/// Decodes one base64 block, tolerating the trailing newline a file may carry.
/// Exactly what the runtime does: `STANDARD.decode`, on the bytes as given.
///
/// Not trimmed. `verify_signature` in the plugin decodes the configured
/// `pubkey` without trimming, so a key with a trailing newline — which is what
/// a file read produces, and what a copy-paste often carries — fails there
/// while a tolerant decoder here calls the pair good. That is the one thing
/// this gate exists to make impossible: a pass that the shipped updater does
/// not agree with.
fn decode_as_the_runtime_does(encoded: &str) -> Option<String> {
    let decoded = STANDARD.decode(encoded).ok()?;
    String::from_utf8(decoded).ok()
}

/// What a signature *file* holds, which is a different thing.
///
/// `tauri signer sign` writes a trailing newline, and the manifest field is the
/// file's contents. Trimming belongs here, at the boundary that reads a file,
/// and not in the decoder the configured key goes through.
fn decode_signature_file(encoded: &str) -> Option<String> {
    decode_as_the_runtime_does(encoded.trim())
}

/// An environment variable that is present and not empty.
fn optional_var(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        Ok(_) | Err(VarError::NotPresent) | Err(VarError::NotUnicode(_)) => None,
    }
}

/// The repository root, from this crate's manifest directory.
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent directory")
        .to_path_buf()
}

/// The public key exactly as it ships, read from the file that ships it.
///
/// Read rather than duplicated so this cannot drift: changing the key in
/// `tauri.conf.json` changes what this test verifies, in the same edit.
fn configured_pubkey() -> String {
    let config = repository_root().join("src-tauri/tauri.conf.json");
    let text = fs::read_to_string(&config)
        .unwrap_or_else(|error| panic!("read {}: {error}", config.display()));
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", config.display()));
    parsed["plugins"]["updater"]["pubkey"]
        .as_str()
        .expect("plugins.updater.pubkey is a string")
        .to_string()
}

#[test]
fn the_shipped_public_key_verifies_a_signature_from_the_release_private_key() {
    let signer = TauriSigner {
        root: repository_root(),
        scratch: PathBuf::from(env!("CARGO_TARGET_TMPDIR")),
    };

    match pairing(&configured_pubkey(), FIXTURE, &signer.sign()) {
        Pairing::Paired => {}
        Pairing::Mismatched(why) => panic!(
            "\n\
             UPDATER KEY PAIR MISMATCH\n\
             The `plugins.updater.pubkey` in src-tauri/tauri.conf.json is not the public half\n\
             of the key this machine signs with: {why}\n\
             Shipping this build would make every update it ever sees fail to verify, with no\n\
             way to repair it except a manual reinstall. Fix the key before releasing.\n"
        ),
        Pairing::Unusable(why) => panic!(
            "\n\
             UPDATER SIGNING KEY NAMED BUT UNUSABLE\n\
             {why}\n\
             A key was configured, so somebody meant this to be checked, and the check could\n\
             not run: a wrong path, a signer that is not there, a password that does not\n\
             open the key. This is a failure and not a skip — passing here would report a\n\
             broken signing setup as a verified one.\n"
        ),
        Pairing::Unverified(why) => {
            assert!(
                optional_var(REQUIRE_PAIRING).is_none(),
                "\n\
                 UPDATER KEY PAIRING NOT VERIFIED, and {REQUIRE_PAIRING} demands it.\n\
                 {why}\n\
                 A release must not be built without proving the shipped public key matches\n\
                 the signing key. Provide TAURI_SIGNING_PRIVATE_KEY_PATH or\n\
                 TAURI_SIGNING_PRIVATE_KEY.\n"
            );
            eprintln!(
                "\n\
                 ==================================================================\n\
                 SKIPPED, NOT PASSED: the updater key pair was NOT verified.\n\
                 {why}\n\
                 Nothing here checked that `plugins.updater.pubkey` in\n\
                 src-tauri/tauri.conf.json is the public half of the release signing key.\n\
                 If it is not, every shipped build rejects every update permanently.\n\
                 To verify, re-run with TAURI_SIGNING_PRIVATE_KEY_PATH set; see the header\n\
                 of src-tauri/tests/updater_key_pairing.rs.\n\
                 =================================================================="
            );
        }
    }
}

#[test]
fn a_signature_from_another_keypair_is_a_mismatch() {
    // The accident itself: a real, well-formed public key that simply belongs
    // to somebody else's private key. `config.test.mjs`'s shape check accepts
    // this key happily.
    let signed = FixedSigning(Signing::Signed(OTHER_SIGNATURE.to_string())).sign();

    assert!(
        matches!(
            pairing(&configured_pubkey(), FIXTURE, &signed),
            Pairing::Mismatched(_)
        ),
        "the shipped key must reject a signature it did not make"
    );
}

#[test]
fn the_matching_public_key_accepts_that_same_signature() {
    // The control for the test above. Without it, a verifier that rejects
    // everything — a wrong encoding, the wrong legacy flag — would look exactly
    // like a working gate, and would then also reject the real key pair for the
    // wrong reason, or worse, be "fixed" by loosening it until it passes.
    let signed = FixedSigning(Signing::Signed(OTHER_SIGNATURE.to_string())).sign();

    assert_eq!(pairing(OTHER_PUBKEY, FIXTURE, &signed), Pairing::Paired);
}

#[test]
fn signing_other_bytes_is_a_mismatch() {
    let signed = FixedSigning(Signing::Signed(OTHER_SIGNATURE.to_string())).sign();

    assert!(matches!(
        pairing(OTHER_PUBKEY, b"different bytes\n", &signed),
        Pairing::Mismatched(_)
    ));
}

#[test]
fn an_unreachable_key_is_never_a_pass() {
    // Whatever went wrong, the one thing it may never be is `Paired`: that is
    // the whole difference between a skip and a green release gate.
    for signing in [
        Signing::Absent("no key on this machine".to_string()),
        Signing::Refused("`tauri signer sign` failed: bad password".to_string()),
    ] {
        assert_ne!(
            pairing(&configured_pubkey(), FIXTURE, &signing),
            Pairing::Paired
        );
    }
}

#[test]
fn no_key_here_is_a_skip_and_a_key_that_will_not_sign_is_a_failure() {
    // A contributor with no release key is the ordinary case, and the question
    // simply goes unasked.
    assert!(matches!(
        pairing(
            &configured_pubkey(),
            FIXTURE,
            &Signing::Absent("no key on this machine".to_string())
        ),
        Pairing::Unverified(_)
    ));

    // A key that was named and did not sign is somebody having meant to check,
    // with the machinery broken under them. Sharing "nobody asked" with this
    // is how a wrong path or a bad password reads as a verified release.
    for why in [
        "`tauri signer sign` failed: bad password",
        "no such file or directory: /nowhere/release.key",
        "failed to run `pnpm`: command not found",
    ] {
        assert!(matches!(
            pairing(
                &configured_pubkey(),
                FIXTURE,
                &Signing::Refused(why.to_string())
            ),
            Pairing::Unusable(_)
        ));
    }
}

#[test]
fn a_public_key_that_is_not_one_is_a_mismatch_not_a_skip() {
    // A garbled or truncated `pubkey` is as fatal to shipped builds as the
    // wrong one, so it fails rather than going quiet.
    let signed = FixedSigning(Signing::Signed(OTHER_SIGNATURE.to_string())).sign();

    for configured in ["not base64 at all!!", "", OTHER_SIGNATURE] {
        assert!(
            matches!(
                pairing(configured, FIXTURE, &signed),
                Pairing::Mismatched(_)
            ),
            "{configured} must not verify"
        );
    }
}

/// A key the shipped updater would refuse must not pass here.
///
/// The gate's whole purpose is that a green run means the runtime agrees. A
/// decoder more forgiving than `verify_signature` breaks exactly that: the
/// trailing newline a file read leaves on a key would sail through the gate and
/// fail on every machine that ever checked for an update.
#[test]
fn a_configured_key_the_runtime_cannot_read_is_a_mismatch_here_too() {
    let signed = FixedSigning(Signing::Signed(OTHER_SIGNATURE.to_string())).sign();

    for trailing in ["\n", " ", "\r\n", "\t"] {
        let padded = format!("{OTHER_PUBKEY}{trailing}");
        assert!(
            matches!(pairing(&padded, FIXTURE, &signed), Pairing::Mismatched(_)),
            "a key with {trailing:?} after it passed the gate; the runtime would refuse it",
        );
    }

    // And the key itself, unpadded, is still good — otherwise the assertion
    // above would pass for the wrong reason.
    assert_eq!(pairing(OTHER_PUBKEY, FIXTURE, &signed), Pairing::Paired);
}

/// A signature file's trailing newline is not the same thing: `tauri signer
/// sign` writes one, and the field is the file's contents.
#[test]
fn a_signature_file_may_end_in_a_newline() {
    let signed = FixedSigning(Signing::Signed(format!("{OTHER_SIGNATURE}\n"))).sign();

    assert_eq!(pairing(OTHER_PUBKEY, FIXTURE, &signed), Pairing::Paired);
}
