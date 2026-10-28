# Independent review of shared immutable source-reset ownership

Frozen clean checkout `/workspace/nessa-agent`, branch `codex/896-webhook-delivery`.
Reviewed head **43a9094623af1b2aac6b0c3211e1b93573941377**, implementation source
**0bf207707f4144ab9634c5ed7764e66fa7b4dcba**. Correction base
**0f6bea1e904267e86cd0ec3ee8a858c75371cec1**. PR1096's intended stack base remains
foundation **6c38915d5201de4cac54500ec4b5ad5e5f34229e** / PR1091.

Outcome: **changes required: one minor and one nit. Blocker and major empty.**
No functional provenance, lifecycle, restoration or authorization defect was
confirmed in this review. This is not a clean review, full-feature acceptance,
canonical-browser qualification or merge approval.

## Blocker

Empty.

## Major

Empty. No new independent structural constraint was found. The parent-adopted
shared original reset owner addresses the prior review's source-provenance gap
within this storage slice. Actor/time snapshots now correlate with the immutable
fact and actual capture membership; they do not manufacture their own original
cause. The direct owed-record query addresses the prior optional-OR selection
scan. These conclusions are limited by the checks and exclusions below.

## Minor: the new source-reset module violates the required Rust organization

Locations: `crates/nessa-server/src/webhooks/infrastructure/outbox/source_reset/mod.rs:16-155`,
especially `Capture` at18 and functions at23,49,62,103,123; fully qualified
`Error`/`ErrorCode` type references at92-94. Test-only function-local imports are
also present at `tests/webhooks/infrastructure/outbox/source_reset.rs:334,646`.

`CODING_STANDARDS.md:562-566` explicitly reserves `mod.rs` for documentation,
module declarations and re-exports, placing structs/functions/implementations
in named files. The entirely new module instead owns its SQL read constant,
Capture representation and read/insert/capture/delivery behavior in `mod.rs`.
The required top-level Rust type import rule at586-591 also applies to the new
capture error matching and practical test-only imports.

Expected: keep this one consistency boundary in `source_reset/`, with its
behavior in a cohesive named file and `mod.rs` limited to documentation,
declarations and re-exports; group `Error`/`ErrorCode` in the owning file's
rusqlite import and place practical test-only imports at their module's top.
Update the module map, reviewed state source list and applicable source manifests
when moving the file. This is a focused organization correction, not a request
for another permission system, lifecycle state or independent reset owner.
The older parent outbox module's pre-existing layout is outside this finding.

Evidence: direct source inspection against the quoted explicit repository rule;
behavioral architecture tests do not implement this complete organization gate.

## Nit: committed evidence does not pass the claimed diff check

Locations:
- `docs/reviews/evidence/webhook-outbox/shared-reset/canonical.txt:126`
- `docs/reviews/evidence/webhook-outbox/shared-reset/isolated.txt:72`

`git diff --check 0f6bea1e904267e86cd0ec3ee8a858c75371cec1..43a9094623af1b2aac6b0c3211e1b93573941377`
returns **2**, reporting `new blank line at EOF` in both newly committed evidence
files. The same exact stack-base command against6c38915d returns2 with the same
two warnings. A clean worktree's unqualified `git diff --check` examines no
committed change and does not establish this gate for the reviewed diff.

Expected: remove only the extra terminal blank lines, preserving every test
result and meaningful log line, then run the explicit intended-base-to-head
check and retain its result. Exact outputs are `diff-check.txt` and
`stack-diff-check.txt`; exit statuses are in the matching `*-exit.txt` files.
No test assertion, timeout, security or acceptance gate should change.

## Independent verification

Own directory: `/tmp/independent-shared-reset-review`. No root checkout edits,
Git writes or Cargo/shared-artifact builds were performed. The harness was
copied with every probe path rewritten to this directory. It compiles actual
production outbox/domain code and actual SQLite/private-filesystem adapters
against the canonical dependency artifacts; only the Target DTO is substituted.
`source-manifest.json` confirms every copied production webhook source file
matches the frozen head byte-for-byte.

- Independent original baseline: **67 tests passed**, zero failures,47.54s,
  `baseline-run.txt` / `baseline.txt`; `compile.txt` records compilation.
- Three additional adversarial/valid-lifecycle probes: **3 passed**, zero failures,
  0.04s, `probes.txt`; `add-probes.py`, `compile-probes.py`, `probes-compile.txt`, `args.json` preserve
  their exact source and compilation context.
- Combined run on that final probe binary: **70 tests passed**, zero failures,
  47.82s, `all-tests.txt`.
- Exact correction and stack diff checks failed only for the two EOF lines above.
- Root remained clean at the exact reviewed head when verification finished.

The additional probes check:

1. **Later Retry cleanup after original cursor removal and audit trim.** Reopen
   with only retained delivery ownership, then settle the original Sending into
   Retry with an intentionally backward cleanup clock. The subsequent source
   refusal keeps the first reset ID, original Live reader/time1002 and distinct
   cleanup time500. Reopen again accepts that feasible history. This tests original
   cause preservation without inferring order from wall-clock timestamps.
2. **Cross-chat substitution to another existing valid reset.** Build two chats'
   captured Sending rows, then substitute the first chat's owed refusal with the
   second chat's actual retained reset ID while keeping the audit FK projection
   consistent. Publication returns Invalid before changing any audit file and
   reopen returns Invalid. This goes beyond a dangling/random-ID case.
3. **Complete physical source cycle.** Exercise A→B→A→B with later cursor
   deletion/replacement and newly queued work while the original A Sending is
   retained. Restart records Unknown with its first reset membership/reader/time,
   not the current cursor's later reset. Cursor replacement is a private storage
   fixture because the runtime reset command is not composed; this is not E2E.

## Inspected scope and agreement checks

Read AGENTS.md, CODING_STANDARDS gates/local review/dimensions/agreement/audit/
organization/DDD/one-current-contract/import rules, architecture, typed DI guidance,
module map, parent ordering tables, previous exact-head report and demonstrations,
current state page and source checkpoints, and supplied current validation and
mutation evidence. Inspected all changed production files plus full adjacent
queue/journal/stored/capacity/schema/publication/restoration/retirement/cursor/
registry/codec/audit paths, application Outbox/causes, domain Delivery and relevant
existing restoration/source/retention/CAS/cancellation/lost-ack tests. Inspected
new tests and earlier admission interfaces/Cedar principal mapping and grant holds.

Relationships and enforcers checked:

- **Original reset authority:** `source_reset::Fact` canonically validates the
  stable identity, registration/chat, original prior scope/position, changed
  physical source and original reader/time. `source_reset::read` ties the fact
  to SQL identity/registration/byte projection. Immutable schema guards and
  current canonical definitions are checked before restored application effects.
- **Actual affected deliveries:** immutable first-capture membership holds the
  delivery identity, original domain state and position. `compatible` permits
  differing logical epochs on the same physical source while refusing foreign
  registration/chat/checkpoint. `delivery` checks original/current identity,
  epoch, position, attempt, timestamp and permitted current state. Existing
  capture is not overwritten by later source invalidation.
- **Audit and histories:** exhaustive `restoration::contextual` handles
  Registration, Cursor and Delivery against retained owning facts. The history
  owner reaches the actual capture state before source-reset projection, keeps
  the first reset and ends at the current domain state. `SavedAudit` correlates
  row keys and reset FK; snapshots are not a parallel cause authority. The
  whole owed batch and relevant histories validate before first file publication.
- **Normal and historical actors:** credential-stop facts remain distinct from
  reset facts. Registration Issue/Stop/Drained use live or bounded secret-free
  retirement facts, including original target/issuer/stop actor/time and exact
  retirement audit. Valid historical deletion/lost acknowledgement is retained.
- **Effects and acknowledgement:** one SQLite transaction captures fact,
  references, members, cursor and audit. Common commit owns retention;
  publication preserves saved bytes and requires one SQL acknowledgement.
  Failure does not imply a new network attempt. Sending recovery records Unknown;
  known response settlement retains its independent meaning. No exactly-once
  remote effect guarantee was introduced.
- **Retention and resources:** cursor/member/audit retain/release triggers and
  indexed actual reference counts preserve facts until their final owner goes.
  Delivery cascade and audit trimming release references before registration
  deletion. Factory/startup count bounds, SQL byte admission, one-member-per-
  retained-delivery, finite domain attempt/history bounds and existing physical
  worker/owner lifetime gates were inspected. Direct published-state selection
  uses the index without the former optional-OR full scan.
- **Policy and identity:** no source correction changes Cedar, credentials,
  grants or application-principal behavior. Earlier admission gathers current
  credential/principal/grant-epoch facts and asks existing Cedar read policy;
  the storage API remains explicitly trusted-fact infrastructure. Its row or key
  does not supply authorization. This is not a new send capability or receiver
  binding and does not implement #1052.

## Supporting evidence and exclusions

Supplied exact-source97canonical webhook tests, seven-package all-target Clippy,
108architecture/checker,27caught/restored mutations,122-page catalog and two
Chromium151 chart renders were inspected as supporting evidence, not independently
rerun or substituted for the probes above. The existing const-evaluation notice
is retained. I did not run Cargo, shared builds, remote CI or browser scripts.

Canonical gateway Chromium/WebKit, default/raised stress and chaos remain applicable
and unqualified here. NessaDocs chart rendering is documentation evidence only.
The denied foundation job120322813252 retry/dispatch/alternate routes were not
attempted. Hosted exact-head CI remains parent-owned; no terminal result is claimed.
No live recipient, registration, persistent credential, production transmission,
public listener, security/network change, merge or force-push occurred.

Registration/configuration admission, principal share resolution, revoke/expiry
coordination, authorized source/replay, supervised sender/live settlement,
capability/revoke/expiry/stale-epoch/cancellation/no-send recipient E2E and runtime
integration with main56602fc6 remain explicitly uncomposed. They are separate
remaining acceptance work, not waived by this storage review. Findings at every
priority need closure before a clean final review of the corrected exact head.
