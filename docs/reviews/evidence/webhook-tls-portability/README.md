# TLS fixture portability correction

Source checkpoint: `b8568beee166680a45a192222da468234933411d`.
Base: PR #1091 head `2613b8f68a6f12d08a74903a14fddce2b3cdba1d`.
This is a test-only correction, independent of dependent admission work.

## Observed failure and correction

Hosted run [39921247731](https://github.com/nessalabs/nessa-agent/actions/runs/39921247731),
macOS job 120306789392, failed six HTTPS fixture tests. The parent supplied
logs showing NotSent rather than the expected response/error and recipient
TLS alerts CertificateUnknown. OpenSSL inspection of the old DER showed a
100-year leaf without server-auth EKU. Both violate
[Apple's certificate requirements](https://support.apple.com/en-us/103769).
The installed rustls-platform-verifier 0.7.0 fixture generator documents these
same macOS constraints. Linux accepting that leaf did not verify macOS trust.

Each recipient now generates a private in-memory test CA and an eight-day
SAN/server-auth leaf. No generated key is written to disk. No production source,
TLS backend, trust policy, DNS/address policy or response/signature assertion
changed. rcgen and time are test-only dependencies; rcgen uses the existing ring
backend, without adding a production crypto provider. The obsolete fixed DER
certificate/key were removed.

The new negative test requires a concrete rustls certificate error for both an
untrusted CA and wrong hostname, and no HTTP bytes at the recipient. Review
found that std::io::Error::source skips its contained error; the test now walks
get_ref first. The first canonical run, built before that review correction,
passed the original 26 checks and failed the new error-walker assertion. The
corrected exact source passes all 27.

## Verification

- Canonical `cargo test -p nessa-server --lib webhooks --no-fail-fast`:
  **27 passed**, zero failed/ignored, 3748 unrelated tests filtered out.
- Required package-selection Clippy, `--all-targets -- -D warnings`, passed
  for local-storage, auth, server, protocol, client-core, device-client and SDK.
- Formatting, architecture and diff whitespace checks passed.
- Rust 1.99.1 is isolated under `/tmp`; the pre-existing forced lease-protocol
  constant-evaluation notice still prints, and the commands exit zero.
- Independent source review found no remaining narrow-scope findings after the
  error-walker correction. A separate typed-error probe detects a nested
  certificate error and rejects ordinary ConnectionRefused.

[`mutations.json`](mutations.json) records two verified changes applied only
to copied source under `/tmp`: accepting invalid certificates, and adding the
wrong hostname to the leaf SAN. Both fail the new negative control. Restoring
the copies byte-for-byte passes all 27 in
[`isolated-restored.txt`](isolated-restored.txt). The isolated harness links the
exact dependencies from the canonical Cargo test fingerprint; an initial
attempt using newest-by-time rlibs failed to compile due to mismatched feature
sets and is not counted as execution evidence.

Local Linux verification is complete. New exact-head macOS/Windows CI is still
required; this evidence does not claim those platforms passed. The full #896
registration/outbox/worker/replay/E2E acceptance remains unfinished.

Nessa Docs `c0bc0627611b100ab71a96cc6fe2fb65f1a014d8` validated 120 state
documents with zero issues. Both existing foundation charts rendered in installed
Chromium 151 with zero page errors and were visually inspected. The reviewed
page now records the exact source checkpoint and the fixture trust distinction;
[`render.json`](render.json) records the source and renderer revisions.
