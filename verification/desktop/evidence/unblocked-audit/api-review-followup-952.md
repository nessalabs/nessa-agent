# API follow-up for PR 952

Reviewed and edited in `/workspace/nessa-950-api-review` on
`codex/thread-events-api`, based on `331ba136ccfda91a7e7c88a4227eabd7c5616897`.

The scoped follow-up delegates event authorization to `check_view_access`,
rejects contradictory terminal cursor evidence and correlates it with the live
subscription kind, asks shared owner-phone command-target admission for peek,
correlates the README settlement wait with the send receipt, and returns event
option validation failures through the promised Promise. The active-execution
fixture now creates its private storage child using `nessa_local_storage::create_directory`.
Ordering rows E18–E20 were written before these changes.

## Verification

- Focused Vitest run: 3 files, 69 tests passed (subscription-api, thread-api,
  thread-subscription).
- Complete client Vitest run: 39 files passed, 2 failed; 982 tests passed,
  2 failed. Both failures are unchanged real private-file positive fixtures:
  `loads only the assigned surface in the matching stage and instance` and
  `reads the real stage and instance namespace with private file checks`.
  The environment's `/` is uid 65534 (`nobody`), whereas
  `validateAcquisitionChain` requires locator ancestors to be root or the caller.
  A repeat with `TMPDIR=/workspace/nessa-client-test-tmp` has the same failures.
  Full repeat output: `/tmp/952-client-suite.log` (local run artifact).
- Client `tsc --noEmit`, changed-file ESLint and Prettier checks passed.
- `node scripts/check-protocol.mjs` passed with rustfmt on PATH.
- Direct rustfmt check passed on all four changed Rust files; diff whitespace check passed.
- Product protocol and architecture entry points require Cargo internally. They
  could not start Cargo (ENOENT); no Cargo process or mutation probe was run.
  Rust tests, full product protocol/architecture checks, and mutation probes
  remain with the coordinating agent, which reserved the shared target.

## Regression and probe handoff

`subscription-api.test.ts` adds `rejects terminal frames combining view and event
cursors` and a 24-case terminal-cursor matrix: all of events/view/list/agents,
before/after open reply, and lastDelivered/lastEventCursor/both. Correct single
cursor evidence remains accepted for the appropriate event or view kind.
`rejects malformed replay options before sending` now asserts Promise rejection.

`owner_phone_delete_retry_returns_the_idempotent_receipt` additionally substitutes
an access evaluator that permits Read and refuses Act, then verifies peek returns
`conversation_not_found`. Reverting only peek's `target_params!` to `params!`
should fail this assertion. Current Cedar already refuses deleted/foreign targets
on the existing Read path; this regression distinguishes the command-target
question itself. The existing active-execution test retains its lifecycle,
truncated-display, provider-open count, and foreign-owner assertions.

After shared verification is available, run the two named Rust regressions,
`cargo test -p nessa-server`, the CI package selection, Clippy with `-D warnings`,
and product protocol/architecture checks. For load-bearing probes, separately
remove the terminal dual-field rejection, remove subscription-kind correlation,
and remove `async` from events; each should fail its focused client regression.
Print each mutation hunk before running it, restore afterwards, and rerun green.
The authorization deduplication adds no second access rule to mutate: its single
owner remains `check_view_access`.
