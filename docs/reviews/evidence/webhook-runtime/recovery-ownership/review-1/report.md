# Retained credential ownership: independent review ONE

Result: clean within the narrow reviewed scope. Blocker: none. Major: none.
Minor: none. Nit: none. This is not final assembled PR or issue #896 acceptance.

Checkout: `/workspace/nessa-webhook-recovery`.
Base: `c2f94625100f6ed018b156c06b08b67a3bd3994d`.
Head: `97f3787ca88a7e06300726a635127897a218dea5`.
The tree was clean before and after review. No repository file, branch, fixture,
or ref was edited. The changed production file retained SHA-256
`935632c2a66651d20f112823fd9a30477807c640df976385560b551f10b1bf11`.

## Code and relationship review

Reviewed AGENTS.md, coding standards gates and local-review requirements,
organization guidance, relevant architecture/module maps, all four changed files,
and adjacent retirement, restoration, publication, journal, registry, store,
schema, and row-budget tests. The source change stays in the existing private
SQLite infrastructure owner. No policy decision, public API, durable state,
lifecycle phase, schema, bound, timeout, or acknowledgement flag is added.

The authoritative relationship is credential identity across retained retirement
owners and still-live registration rows. `retirement::by_credential` (lines
103–134) retains the existing at-most-two indexed retirement selection and adds
an indexed live-row existence check. It rejects duplicate retained owners and
live/retired overlap by exact CredentialId text, independently of registration
ID or principal attributes. The domain identifier preserves original text without
normalization; the schema uses the corresponding exact credential equality.
The live check includes stopped-but-not-retired rows, as required for retained
ownership, and does not reject a live registration when no retirement exists.

`retirement::validate` (lines 146–190) now asks that same selector for every
bounded retained row and compares its selected registration identity. It still
validates row bytes before owned decoding and still checks the exact Drained
audit identity and absence of the same live registration ID. The scan remains
capped at MAX_REGISTRATIONS plus one sentinel; per-row credential lookups use
the canonical schema indexes rather than whole-history searches.

Cold `SqlOutbox::open` calls restoration validation before audit-directory setup
or stale audit-temp cleanup. Both restoration paths call `validate_records`,
which validates retirements before selecting audit records. Live publication
calls `validate_owed` before its first audit file access or SQL acknowledgement.
`finish` publishes before trim/retirement; physical operations hold the existing
connection mutex, so no adapter mutation interleaves between these checks and
their effects. The registry stop preflight's live-first selection does not bypass
the correction: `run_checked` subsequently reaches `finish` and the shared
publication validation before its mutation closure.

Checked contradictory complete retirement/audit pairs, intent-consistent live
overlap, distinct credentials, owed Drained acknowledgement with the original
file already present, refusal before publication/trim, cold refusal, and later
repair with exact original audit-byte recovery. Original target, first stop,
issuance time, audit identity and acknowledgement remain separate facts; the
correction changes none of their transition owners or projections.

## Independently executed tests

From the reviewed checkout:

```sh
CARGO_HOME=/workspace/nessa-toolchain/cargo RUSTUP_HOME=/workspace/nessa-toolchain/rustup PATH=/workspace/nessa-toolchain/cargo/bin:$PATH cargo test --locked -p nessa-server --lib webhooks:: > /tmp/nessa-896-retained-review-1-tests.log 2>&1
git diff --check
git status --porcelain
sha256sum crates/nessa-server/src/webhooks/infrastructure/outbox/retirement.rs
```

The canonical webhook suite passed **174/174**, zero failures/ignored, in 17.67s.
This independently executes the four new live/cold ownership regressions and
their positive controls, plus existing general-stop, original-fact, row-bound,
indexed-lookup, restoration, publication-loss, reset and retention regressions.
The diff check passed and the tree remained clean.

Read the author's three preserved mutation logs separately from this independent
run: baseline and shared-validator removal each show four rejection tests failing
because actual refusal was absent; overlap-check removal shows both overlap tests
failing while both duplicate-retirement tests pass. I did not rerun mutations or
alter the frozen source. Author Clippy/architecture evidence is not relabeled as
an independent run here.

## Independent SQL query probe

Source: `/tmp/nessa-896-retained-review-1-query-probe.py`.
Log: `/tmp/nessa-896-retained-review-1-query-probe.log`.

```sh
python /tmp/nessa-896-retained-review-1-query-probe.py > /tmp/nessa-896-retained-review-1-query-probe.log
```

Using the exact canonical schema in disposable in-memory SQLite, the new live
existence query uses `sqlite_autoindex_registrations_2` as a covering credential
index at both 1 and 64 live rows. Exact credential matches remain present for
both Active and stopped SQL rows; different credentials and different-case
opaque IDs remain distinct. These are query-mechanics controls only: the probe's
minimal rows are explicitly not Auth provenance or complete restorable fixtures.
The Rust suite supplies the complete semantic fixture evidence above.

## Exclusions and remaining gates

No full assembled PR review, runtime registration/shutdown supervision,
pending uninstalled-intent recovery, source/share/sender composition, recipient
capability/revoke/expiry/retry/cancellation/no-send E2E, platform/MSRV execution,
browser qualification, hosted CI qualification, or full issue completion is
established. The unpublished cancellation source and its historical dirty M1
variants were not recovered and were not reviewed. This round cannot retire
findings against those unavailable variants. No denied workflow/browser/board
operation or external network transmission was attempted.
