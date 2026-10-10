# #690 publication test receipt gates — author handoff

Checkout `/Users/nessa/.codex/worktrees/unblocked-panel-list/nessa-agent`; branch `codex/690-publication-test-waiters`.
Exact clean base `05bc3c6c3b1ab1a9148bfc45f7cb133cc6f855fe`; clean source head `b323d2c4825f92b48cc927d09e93add338589ea0`.
Two files only,4 insertions/3 deletions: existing application test gates and existing ADR689 test-fixture ordering row. No root, shared target, GitHub or push mutation.

## Actual cause and scope

Actual required Windows CI at05bc failed the identical in-flight caller test at its second-error assertion;427 other cases passed. Source blame independently attributes the changed caller ownership to `68f53229ddd7a25b8b35d280892e5ca9ca12089d` (`Keep Claude directory publication owned through native settlement`), introducing service.rs1070 first-caller wait_claude_publication after spawning the retained transaction. Original965 first caller reconciled synchronously and did not contribute a receipt waiter; old test gates persisted.

Existing ClaudePublication::wait increments its existing cfg(test) counter before waiting on the same retained outcome. The first caller alone can therefore make count1. Three two-caller tests previously released their held native/settings effect at that first count, before proving second caller admission. A later second call legitimately starts a new publication after the failed first one settles, so expecting its old failure is a test-order contradiction, not a platform failure to dismiss.

Change the three existing conditions from ==0 to <2: identical caller shares the held failed receipt; different directory waits for it before starting its own; unchanged durable save keeps the next caller excluded until settlement. Preserve all original outcome/host/settings/attempt assertions, timeouts and pacing. The fourth gate in application/claude_publication.rs intentionally witnesses the sole first caller before panic/drop and remains byte-identical ==0. No new production counter/marker/probe/helper/flag/API/state/policy is introduced. The ADR row was written before code and explicitly describes test-fixture release rather than a new product policy; shortening kept existing Markdown column widths.

## Deterministic public semantic proof

Scratch-only injected scheduling, removed from final source: wait for the first registered receipt; hold second before its public change_claude_configuration call on a test-local sync channel. Admit second from the existing two-caller waiting-loop body; if a broken gate skips that body, admit it only after first.join has settled. Same scheduling scaffold and original public semantic assertions across all four stages. No fixed timing delay or production hook creates admission authority. Native effects and returned typed outcomes come from the existing public test path.

| Source stage | Selected result | Typed first / second | Authoritative directory / native attempts |
| --- | --- | --- | --- |
| Exact05bc old gate with controlled schedule | exit101,0/1 | Gateway(Registration) / Ok | requested /2; original second-error assertion fails |
| Fixed three gates, same schedule | exit0,1/1 | Gateway(Registration) / same failure | None/restored /1 |
| Remove only identical <2 gate to ==0, other gates stay fixed | exit101,0/1 | Gateway(Registration) / Ok | requested /2; same original assertion fails |
| Restore fixed bytes with freshmtime, same schedule | exit0,1/1 | Gateway(Registration) / same failure | None/restored /1 |

The proof distinguishes receipt admission from a legitimate later attempt through actual typed results and host state. Debug output retains enum/cause meaning; no decision branches parse error text. Failure101 is a behavioral assertion failure after successful compilation, not a compile failure. Zero-selected auxiliary binaries are excluded from counts. Initial private native build took84s; stage logs, actual source snapshots and before/fixed/removal/restoration SHA256 assertions are adjacent. Revert occurrence1→0 and byte/freshmtime restoration were asserted. The final source restoration removes all injected scheduling/debug lines and leaves only the three conditions.

Neighboring gate removals were not attempted, and no claim is made that late admission must fail their final positive semantics. The identical case is the source-attributed failing shared setup-rule witness; adjacent different/unchanged cases prove their permitted trailing outcomes. No redundant committed counter assertions or new interleavings were added to force individual neighbors to fail.

## Final-source checks

- Canonical `cargo test -p nessa-app --no-default-features --bin nessa-app gateway::application::service::tests`:61/61 pass,0 ignored,645 filtered,0.12s test execution. Includes all three affected tests, dropped/panicking callers, and unchanged sole-first-waiter gate neighbors. These61 are not claimed as the complete host suite.
- `cargo fmt -p nessa-app -- --check`: pass.
- `cargo clippy -p nessa-app --no-default-features --all-targets -- -D warnings`: pass,57.95s, in own private target.
- Existing ADR Prettier and git diff --check: pass.
- Final-source manifest records exact hashes, clean head, scratch removal and unchanged fourth gate. Production src/src-tauri/src/crates/packages source is byte-unchanged; only tests+owning doc differ.

## Self-review and handoff limits

Reviewed the entire two-file diff against canonical current review/agreement/organization gates. One retained publication owns both caller outcomes; the existing test observation must count both known receipt consumers before releasing its held fixture. Current publication identity cannot be replaced while that fixture is held. This changes test sampling, not product admission or completion. Types, seams, fixture ports, durable/live configuration agreement, module paths/maps, MSRV and ABI remain unchanged. No findings remain in author self-review; both sides of the actual identical-caller boundary are proven, and neighbors retain accepted results.

Fresh independent coordinator review and combined full host check are pending. Actual renewed Windows/supported-platform CI is required; this macOS private run is not its replacement. Final05bc browser whole23/23 groups990/990 remains separately source-attributed: this correction changes no browser/product source. Existing33/35 performance failure is not waived or reclassified. Prior native/API source reviews remain their historical regional evidence; this narrow CI test follow-up does not create a whole-code approval.

All owned cargo/native witness/fmt/Clippy processes ended before handoff. No browser or GUI was started; checkout clean. Ready for independent review and integration, not a claim that Windows CI has already passed.
