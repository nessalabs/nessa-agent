# Shared immutable source-reset correction

Source checkpoint `0bf207707f4144ab9634c5ed7764e66fa7b4dcba` implements the
[parent-adopted alternative](../total-context/review-1/parent-adopted-alternative.md)
after the [preserved review](../total-context/review-1/report.md). It remains an
uncomposed storage adapter. No production source reader, registration admission,
sender or capability/revocation E2E is claimed here.

One immutable reset fact owns original identity, scope, reader and time. Cursor,
immutable delivery membership and audit references retain it. Capture includes
the original delivery state, allowing causal history checks without inferring
order from timestamps. Later cursor deletion/replacement cannot overwrite the
first reset of an admitted delivery. Reference cleanup and bounded capacity are
transactional. Live owed-record selection uses the published-state index directly.

## Exact-source validation

Linux cloud workspace, Rust1.99.1. Required seven-package all-target Clippy
`-D warnings` passed in71s ([log](clippy.txt)); the existing lease-protocol
const-evaluation notice is retained. Architecture108tests/checker, formatting and
diff checks passed. The copied-source isolated outbox suite passed **67tests**,
zero failures,54.53s ([log](isolated.txt)). Canonical `cargo test -p nessa-server --lib webhooks --no-fail-fast` passed
**97tests**, zero failures,3749filtered,43.04s ([log](canonical.txt)).

All [27 fail/restore probes](mutations.json) caused their intended regression to
fail, then passed the affected test after source restoration. The [run](mutation-run.txt)
and [script](mutations.py) preserve exact rules. Probes cover immutable guards,
reference cleanup/accounting, canonical schema, early and restoration capacity,
SQL byte admission, original integer/canonical encoding, identity/actor/time/scope/
position correlation, capture ordering, original refusal time, direct selection,
storage failure classification and capture identity/epoch/position/attempt/time.
The [manifest](source-manifest.json) confirms the harness source matches the
checkpoint byte-for-byte; all mutation copies were restored identically. The
[harness](isolated-harness.py) substitutes only Target DTO and uses canonical
compiled dependencies. It is not Cedar/recipient E2E.

The [17 focused reset tests](https://github.com/nessalabs/nessa-agent/blob/0bf207707f4144ab9634c5ed7764e66fa7b4dcba/crates/nessa-server/tests/webhooks/infrastructure/outbox/source_reset.rs)
include later cursor deletion/replacement, older logical epochs, retirement and
lost-ack recovery, transaction rollback, missing/cross-record references, original
metadata substitutions and count/byte bounds. They use actual private SQLite and
filesystem storage. The direct owed-selector test measures zero fullscan steps
and constant VM work with0,100and10000published rows; its selector-only synthetic
rows do not claim valid domain histories.

Self-review preserved four uncovered assertions at the earlier `f428be7b` source:
[immutable-member trigger](initial-uncaught-mutation.txt),
[early cursor capacity](initial-capacity-probe.txt),
[direct fact ID read](initial-direct-read-probe.txt) and
[historical cursor position](initial-historical-position-probe.txt).
A nonexistent reference was rejected by FK even without the immutable trigger;
late transaction capacity masked early admission; other contexts masked direct
ID and historical-position checks. The final regressions isolate those owners.
These were coverage findings, not a new allowed delivery or permission change.
The prior source's96canonical and66isolated passes do not qualify the newer source.

NessaDocs [catalog validation](docs-validation.json) passed122pages/0issues.
Both diagrams rendered in Chromium151 with0pageerrors and were visually inspected;
[render data](render.json), [operation chart](diagram-1.png), [reset chart](diagram-2.png).
Only this owning state page advances its sourceRevision. This is documentation
validation, not canonical gateway Chromium/WebKit qualification.

## Review status

Fresh independent review is pending. No persistence approval or full feature
acceptance is claimed. The runtime worktree remains unchanged until persistence
is clean. Main56602fc69d0dc50868f68cb0bb485bed1b7de2be (including #1097/#1098)
was fetched and will be preserved before composition. No shared auth/grant/protocol
or runtime files changed in this correction.

Only synthetic keys and private filesystem fixtures are used. No live recipients,
registrations or production data are involved. The inherited denied foundation
CI retry remains untouched. Canonical Chromium/WebKit, stress and chaos gates
remain applicable and unqualified; see the [validation handoff](../../webhook-validation-gates/README.md).
