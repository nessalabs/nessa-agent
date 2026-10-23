# Canonical credential-stop correction

Source: `c2bbb2705fe03ec77efc727460893cbadd264639`, based on published
`c7fe656af15346a21c293eb9a5bb5d5cf27c54b0`. This is the inner stop adapter and
use case, not runtime mounting or completed issue #896.

Auth owns retained revocation decoding. A retained shape and a current-bound
revocation are separate types. One bounded stop representation carries original
Auth identity, cause and actor through live, retired and audit state. Local first
observation milliseconds remain separate from command/effective seconds. The
actual-source use case keeps the existing snapshot hold through its storage call.
It supplies no delivery authority.

## Author checks

- Full actual Auth suite: 214 unit and 68 integration tests passed on Linux,
  Rust 1.99.1, `umask 022`. Ordinary credential behavior remains in that suite.
- Canonical server webhook suite at this source: 169 passed. Required seven-
  package all-target Clippy `-D warnings`, formatting and architecture passed.
- Private actual-source owner/domain suite: 139 passed at this source. The
  oversized public-read control did not demonstrate a mapping defect.
- Six deliberate author mutations were caught: different original record,
  prepublication check omission, borrowed row byte-limit omission, substituted
  observation clock, ignored actor projection and early held-snapshot release.
  Only disposable private copies were mutated, and source copies were restored
  afterward.
- Architecture passed with the configured Rust PATH. The first invocation failed
  `spawnSync cargo ENOENT`; that shell setup failure remains recorded separately.
- Nessa Docs catalog: 135 documents, zero issues. All three outbox diagrams rendered
  and were visually inspected against this source checkpoint with system Chromium
  151.0.7922.173, renderer `c0bc0627611b100ab71a96cc6fe2fb65f1a014d8`;
  zero page errors. This is documentation rendering, not canonical browser proof.

The initial Clippy attempt named a nonexistent package and did not execute its
check. The actual seven-package run found an oversized audit enum and a test lock
scope. The optional delivery stop is now boxed without changing serialized bytes;
the test guard now has a lexical scope. Neither lint was suppressed.

## Remaining gates

Fresh independent exact-head review and publication are pending. Windows, macOS, MSRV, exact-head CI, canonical scripted Chromium/WebKit,
default/raised stress and chaos, and the unfinished runtime/recipient E2E are not
qualified by these Linux checks. The prior Windows enrollment assertion and
cancelled jobs remain distinct evidence; there is no demonstrated Windows fix.
Pinned browser download and foundation CI retry denials remain binding. No retries,
alternate triggers, live registrations, production listeners or real recipients
were used.
