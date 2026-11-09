# Independent retained-progress structural review

Reviewed clean frozen **e8f1564f90e34f77bee6858328f34d94a323c01e** in
`/workspace/nessa-webhook-composition`, against published PR1101 base
**c5bdd8e6dc31047cba99773148be61d80b01bc29**. The structural correction is
source **e08b90cf01ecd2fef3e78ce4b707935e1fb2abfa**, with the focused structural
delta additionally compared to **ec19afc0de5000d6cae990ee67787f3e23d7478e**.
The checkout remained clean and unchanged throughout review.

**Blocker 0; Major 0; Minor 0; Nit 0.** No open finding in the reviewed W24
lifecycle/persistence scope. The concrete prior M1/M2 and both M3 cases are
fixed at this head. This qualifies the reviewed inner lifecycle correction,
not completion of outbound delivery or its remaining merge gates.

## Scope and agreement checks

Read AGENTS, the coding standards' gates/review/agreement/audit/resource and
organization requirements, architecture/module guidance, dependency-injection
guidance, the implementation decision table, affected state page and legitimate
preserved lifecycle reviews. Inspected the complete W24 production diff against
its published base: domain lifecycle/value exports, application mappings, audit
encoding/decoding, intents, queue admission, registry lifecycle writes, journal
cleanup, restoration, retirement, schema and the new retained-progress module.
Traced its unchanged collaborators: publication, stored delivery/history and
cursor validation, reset fact/member/reference ownership, capacity and the
private store's serialized retained physical operation. Reviewed corresponding
domain/lifecycle/issuance/schema/cleanup tests and evidence dispositions.

The shared `restoration::progress::validate` replaces the partial current-row
predicate. Cold restoration checks retained initial families; live publication
checks registrations with owed initial records before file effects. Each affected
registration is summarized once, using registration-leading audit selection,
rather than rescanning its retained history for every sibling owed record.
`SavedAudit::checked`, `Record::decode` and existing `contextual` checks validate
identity/target/time/actor/reset relationships before audit classification. Both
Record and Cause matches are exhaustive. Initial identity multiplicity belongs
to this one summary, not a second permission or acknowledgement owner.

The initial publication condition correlates one pending identity, current Active
state and absence of contradictory retained progress. Published initial evidence
excludes a competing pending identity. A genuine existing file is distinct from
SQL acknowledgement and remains retryable under the same identity. A missing or
erased proof does not establish historical acknowledgement; no post-trim monotonic
history or exactly-once remote effect is claimed. This is the parent's A contract,
not B's new durable acknowledgement semantics.

| Persisted family | Checked relationship in the reviewed design |
| --- | --- |
| registration_intents | Existing intent validator binds original selected input/configuration and installation; intent presence alone is not acknowledgement |
| registrations | Existing decoder owns phase/observation; non-Active contradicts a pending initial obligation |
| cursors | Registration-leading current ownership is later work, including cursor-only retained state |
| deliveries | All phases are later work; current/history validators retain target, source and stop context, including valid later Sending/Retry outcomes |
| source_reset_facts | Retained immutable reset is later work after current cursor/delivery deletion; existing decoder/reference owner remains authoritative |
| source_reset_members | Registration derives through its referenced immutable reset/delivery owners, with existing foreign-key/reference/capture validation; it has no independent registration identity |
| audit | Existing validation precedes the exhaustive initial/later classification; UUID multiplicity and published/pending state are correlated |
| retirements | Existing original-target/stop/time/exact retirement identity validation remains; retirement contradicts pending initial publication |

The schema-inventory test fails when an unclassified table is added. The new
registration-leading audit index and the existing keyed work predicates are
exercised at the unchanged 1,024 retained-row limit. Streams retain one audit
record at a time; the registration set is bounded by live plus retired capacity.
Existing source/reset/cursor/pending/terminal limits and flush/acknowledgement
ordering were not relaxed. I inspected the metadata probes as query-work tests,
not as claims of restorable authorization history or full-capacity application
throughput benchmarks.

## Independently executed evidence

All compilation used **private copied source and immutable dependency snapshot**,
not shared Cargo/target writes. Actual current Auth was compiled from the reviewed
source. The small isolated driver assembles the actual changed domain/application/
outbox modules with the unchanged four-field Target representation; it is not a
full server/product runtime build. `source-provenance.json` verifies every copied
production Auth/webhook file byte-equal to the frozen checkout after controls.

- Actual Auth unit suite: **214 passed**, `unit-tests.txt`.
- Private owner/domain baseline: **130 passed**, `baseline.txt`.
- Three additional adversarial/adjacent probes: **3 passed**, `probes-tests.txt`.
- Final restored combined suite: **133 passed**, `final-tests.txt`, including
  unchanged full-boundary terminal/failed-publication/reset retention tests,
  both M3 rejections, prior M1/M2 cases, cold/live no-publication controls,
  genuine file-written/ack-lost recovery, proof erasure and indexed query checks.
- Four independently compiled mutations were caught and byte-restored, with each
  corresponding restored control passing (`controls.json` and per-rule logs):
  bypassing the shared progress call; disabling initial UUID multiplicity;
  removing registration scoping; adding an unclassified canonical schema table.
- Full-base whitespace check passed. Final checkout status is clean at the exact
  reviewed head. No source, shared build artifact, branch or remote was changed.

New probes preserve an actual captured Sending delivery, its reset fact and member
while a different registration's genuine initial publication loses its acknowledgement.
They assert that those owners still exist after ordinary cleanup, cover file-written
and file-not-written failures, cold-restore the new pending registration and resume
its own queue. Thus cross-registration evidence does not become another target's
proof. Other probes substitute a self-consistent initial actor/owner and a valid
but wrong actor in an actual retained published Cursor; existing original-target/
reset correlation rejects them before acknowledgement or cold restoration.

The first exploratory cross-registration probe passed but ordinary cleanup erased
its intended reset-only proof before the new installation. Its source/log are
preserved as `initial-probes-with-cleaned-proof*`, and it is **not counted** as
that stronger evidence. The corrected Sending/reset/member probe explicitly
asserts retention and is the final executed probe. This was a fixture-evidence
correction, not a production failure or a change to the reviewed checkout.

## Dispositions and limits

M1's phase/owed-observation/Queued contradictions remain rejected without rejecting
valid late Sending outcomes. M2's owed initial obligation after observed expiry,
current work or retirement remains rejected. M3's retained reset/Cursor history
with current rows removed and competing initial UUID beside the original are now
rejected by the complete shared retained summary. Valid adjacent published history,
retirement, trimmed rows and genuine initial acknowledgement loss remain accepted.
The new structural correction is a complete inventory-based owner for the adopted
retained-proof contract, rather than another isolated root-presence exception.

I inspected the source checkpoint and scoped docs/evidence claims. Author-reported
canonical server160, seven-package Clippy/fmt/architecture and135-document/three-
diagram checks are supporting evidence, **not independently rerun here**. The docs
explicitly retain the post-trim proof limit and unfinished product/runtime work.
Historical reports/logs remain historical, not current passing gates; not every
historical raw artifact was re-executed.

Unverified: Windows/macOS platform behavior and CI package/feature combinations,
MSRV execution (private checks use installed Rust1.99.1), canonical scripted
Chromium/WebKit, feature-specific default/raised stress/chaos, runtime command
ownership/general revocation mapping/source/sender composition, and actual
capability/revoke recipient E2E. Existing synthetic owner tests do not substitute
for those gates. This review did not dispatch/retry CI, download browsers, mutate
GitHub, use live credentials/register providers, expose listeners or transmit to
real recipients. No denied operation was retried and no blocker remains to
returning this new review. The unrelated flagged old report was not accessed or
reconstructed.
