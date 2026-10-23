# Canonical credential-stop correction

Current source: `342e21aa5250eb97c79e57a1bd56bd10c12a439b`, based on published
`c7fe656af15346a21c293eb9a5bb5d5cf27c54b0`. This is the inner stop adapter and
use case, not runtime mounting or completed issue #896.

Auth owns retained revocation decoding. A retained shape and a current-bound
revocation are separate types. One bounded stop representation carries original
Auth identity, cause and actor through live, retired and audit state. Local first
observation milliseconds remain separate from command/effective seconds. The
actual-source use case keeps the existing snapshot hold through its storage call.
It supplies no delivery authority.

## Author checks at preceding source c2

These checks establish the preceding implementation, not the final M1 repair.



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

## Independent review and retirement identity correction

[Round ONE](review-1/report.md) reviewed frozen `0aa1a479ad6a81ad088fb5b368b9c013d3812ab7`
and required changes: Major M1, test import Nit N1, and composition wording Nit N2.
The report is preserved unchanged. M1 exposed an inherited restoration read that
cloned a malformed retirement ID before checking its UUID. The selected correction
includes the ID in the single borrowed-budgeted `SavedRetirement` projection used
by restoration, lookup and credential selection; restoration no longer has a
separate unbudgeted identity read or rereads the same retirement. Limits, indexed
`LIMIT 2`, original fact semantics and acknowledgement ordering are unchanged.

The desired allocation probe [fails before](review-1/outer-id-before.txt): a
malformed 1 MiB identity caused a 1,048,576-byte Rust allocation before `Invalid`.
Against the correction it [passes](review-1/outer-id-after.txt), with largest
allocation 196 bytes and the same typed refusal. This measures Rust allocations
around the validator, not SQLite page/storage allocations. The committed
[retirement identity and maximum-row tests](../../../../../crates/nessa-server/tests/webhooks/infrastructure/outbox/stop_rows.rs) checks borrowed
refusal and a valid adjacent identity; the maximum stop-size control also decodes
a valid complete retirement. Omitting the ID from that budget [fails the
regression](review-1/outer-id-mutation.txt). Private source was restored afterward.
N1 groups the new test imports at module scope; N2 states that future composition
**must** retain the inner observer, rather than claiming a supervisor exists.
Round TWO below reviews these dispositions and the complete correction.

At source `d742cf55ef740716a7a8dfebe5eaea427819138c`, the canonical server webhook
suite passed **170/170**. The first local canonical compile was killed by SIGKILL
before tests; the cgroup reported one OOM kill (no pre-run baseline). After private
compiles ended, serial Cargo jobs and `MALLOC_ARENA_MAX=2` completed the same test
command without another OOM kill. No assertions, timeouts or production limits
changed. Private owner baseline140 plus the allocation probe passed141; six prior
mutations and the new identity-budget omission were caught and restored. The Auth
source is byte-identical to preceding c2, whose full282 tests passed. Formatting,
architecture, docs135/0 and three rendered/visually inspected diagrams passed at
the new source. Required seven-package all-target Clippy `-D warnings` passed;
that source then entered round TWO.


## Fresh round TWO and complete import closure

[Round TWO](review-2/report.md), frozen at `ef06f84f0f355b631e29a8da91d53f688499967f`,
verified M1 and N2 fixed and found no Blocker, Major or Minor. It retained Nit N1:
eight new qualified type expressions/signatures across registry, retirement and
two test files. Source `342e21aa5250eb97c79e57a1bd56bd10c12a439b` imports these types
at each owning module and uses short names. This is an import-only correction;
no lifecycle, authorization, persistence or network behavior changed.

Round TWO independently passed143 owner tests,214 Auth unit tests,8 revocation
tests and docs135/0. It caught/restored seven meaningful rule mutations, including
current-Credential binding. Its six oversized identity-field controls all returned
Invalid with largest measured Rust allocation196bytes; the excluded-ID mutant
allocated1048576bytes and failed. The complete original report records private
assembly, controls and limits; these are not relabeled canonical server checks.
At source342e21aa, the repeated canonical webhook suite passed170/170 and required
seven-package all-target Clippy `-D warnings`, fmt and architecture passed. Docs
validation passed135/0 and all three diagrams rendered and were visually inspected
at the new owning checkpoint with zero page errors. There was no new OOM kill.
Auth implementation remains byte-identical to the full282-test preceding source.
[Fresh round THREE](review-3/report.md) is clean at exact candidate
`ab5caa5bb28dd5a2c419f44f218999c8ec6ee910`: all four priority buckets empty.
It independently passed145 owner tests,214 actual Auth unit tests,8 revocation
integration tests, five final restored controls and docs135/0. Seven rule mutations
were caught/restored; all119 production files matched frozen source. It verified
the actual-validator ID-budget mutant allocates1048576bytes versus196 restored,
with the same typed Invalid. This closes the narrow correction's local review
loop. Later report/evidence closure changes contain no production or test changes.

## Remaining gates

Publication and exact-head CI are separate qualification steps. Windows, macOS, MSRV, exact-head CI, canonical scripted Chromium/WebKit,
default/raised stress and chaos, and the unfinished runtime/recipient E2E are not
qualified by these Linux checks. The prior Windows enrollment assertion and
cancelled jobs remain distinct evidence; there is no demonstrated Windows fix.
Pinned browser download and foundation CI retry denials remain binding. No retries,
alternate triggers, live registrations, production listeners or real recipients
were used.
