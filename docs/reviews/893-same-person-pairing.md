# Same-person pairing: implementation and verification

Issue: [#893](https://github.com/nessalabs/nessa-agent/issues/893).
Base: `19d631ebd34c6ece0062cbd0edbbd5e269d54c1d`.
Source checkpoint: `a3dd63839d52a616560cc70eca470f6a82dbb86f`.
The later documentation commit records these receipts; it does not change the source.

## Scope and ownership

The base contains #889 through #904 (`a70070f6045f6e72b568eff0546f2c0c07b2a6ef`,
“Make a conversation share one access grant”). It had the unified capability row,
but still kept principal-keyed grants in conversation metadata. #893 needs one
receiver-bound principal fact and the design's independent, versioned grant file.
The remaining prerequisite was unowned: #892 had no assignee, and open branches
#1282, #1283 and #1284 contained no grant-store implementation.

Ownership was checked again before publication. #1282 at
`9bd3e7c2477684b75867fb7249eaa34453376509` shares only the protocol schema and its
two generated products; its separate `ConversationMessage.retried` hunks do not
overlap `PairingCreateParams.samePerson`. Its UI, styles and stress scripts were
not changed here. The native catalogue fixture used by stress now explicitly
revokes its former receiver when replacing the target; receiver grants no longer
silently replace one another. Its assertions and deadlines are unchanged.

The project-board lookup was refused by the integration with `Resource not
accessible by integration`. Issue descriptions were updated; board status was
not changed. No merge, deployment, real device pairing, live credential, account
grant or paid resource was used. Every credential in the tests belongs to a
throwaway local namespace.

## Contract and enforcers

The [canonical contract](../design/access-grants.md#same-person) owns the ordering.
The [scoped state page](../state/services/gateway/same-person.md) records its source
checkpoint without extending the historical device-pairing model's claims.

| Boundary | Enforcer and exercised evidence |
| --- | --- |
| Peer-only immutable choice | Consent/PublicIntent values, retained invitation, peer PAKE context, required peer wire field, private binary and peer JSON record; false and true relabelling refused |
| Active precedes attribution | Registry activation publication, then `PairingPersonLinks`; insert/audit trigger failures leave canonical Active plus interruption, and retries reuse the credential and receiver |
| Attribution adds no authority | Unchanged Cedar bundles and credential classes; unshared reads refused; comment/drive refused; admission retains actual principal, capabilities and grant epoch |
| Exact credential correlation | `PersonLinkChange::from_record` validates related credential, principal, organization, audience, grants and original revocation together; SQL admission joins exact credential and organization |
| One grant owner | Existing SQL adapter attaches schema-1 grant file; receiver/principal tagged subjects and export identities preserve independent grants |
| No migration | Foreign grant version refuses before metadata opening, bytes unchanged; readonly snapshots check version/shape; legacy metadata grants remain unused and unchanged |
| Cleanup ordering | Registry commits revoke/cancel before effect; audit and unlink one transaction; fixed expiry/revocation times preserve original boundary and initiator |
| No stale inheritance/replay | Old unlink must commit before same-key replacement, including false choice; exact-credential cleanup preserves newer links; durable removal receipt prevents replay after clock rollback/reopen |
| Expiry before initial insertion | Missing link still commits an absent-before/after removal receipt; failed receipt remains owed, restored write consumes the choice before clock rollback |
| Composition | Root binds cleanup with native disabled and no agent; actual two-gateway activation/restart retains choice; access dataset outside conversation directory |
| Backup file selection | Both namespace capture and committed-fence inventory use explicit independent path; valid sibling decoy regression catches accidental default-path reads |

The registry mutex serializes reconciliation with credential writes. Grant work
keeps that serialization through the physical SQL transaction. Credential holds
use the admission read fence; synchronous registry writers only try the write
fence while holding the mutex, and async revoke acquires its write fence before
the mutex. The source callback uses the canonical mutex, not the published
snapshot lock. This avoids waiting on an admission writer while holding the
mutex needed by a reader's grant write.

## Verification receipts

At source `a3dd63839d52a616560cc70eca470f6a82dbb86f`:

| Check | Observed result |
| --- | --- |
| `cargo fmt --all -- --check`, `git diff --check` | Passed |
| Clippy, all targets, warnings denied | Passed for all nine packages below |
| Full Rust runner, concurrency 2, partition all | Passed; 8,983 reported passes, zero failures, 42 ignored across 57 target/child/doc-test result lines |
| Server main suite | 4,796 passed, 17 ignored, including the repeated replacement load scenario |
| Native enrollment suite | 139 passed |
| SDK unit suite | 1,612 passed, 16 ignored; application/domain/infrastructure and remaining integrations also passed |
| `node scripts/check-sdk-docs.mjs` | Passed |
| `cargo build -p nessa-server` | Passed; compiler emitted its long constant-evaluation warning |
| Final browser suites | Scripted production pass; selected columns stress pass; sidebar stress setup failed (details below) |

The exact Rust package set was `nessa-local-storage nessa-auth nessa-server
nessa-protocol nessa-client-core nessa-device-client nessa-sdk nessa-timer
nessa-vault`. Clippy used `--all-targets -- -D warnings`; tests used
`node scripts/cargo-test-parallel.mjs --concurrency 2 --partition all -- -p ...`
with each of those packages. Logs: `/tmp/893-{fmt-final,diff-final,clippy-final,
rust-final,sdk-docs-final,build-server-final}.log`; aggregate result-line counts:
`/tmp/893-rust-final-summary.json`.

Final production scripted receipt (`/workspace/scratch/893-scripted-head/pr-summary.md`):

> Verdict: pass. Relevant log lines: none.

| Check | Chromium | WebKit |
| --- | --- | --- |
| resize-observers | pass (both layouts; four total checks) | pass |
| mcp-apps-gateway | not run: dev server only | not run: dev server only |
| gateway-window | pass (11 total checks) | pass |
| scripted-scenarios | pass (eight total checks) | pass |
| parent-stop | pass | pass |

This was `pnpm test:e2e:scripted --mode prod --channel bundled --evidence
/workspace/scratch/893-scripted-head`. Raw stdout/stderr is
`/tmp/893-scripted-head.log`; result JSON, screenshots and provider/gateway logs
remain under that saved evidence directory. The final fitted Chromium screenshot
was also visually inspected. Gateway-window and scripted scenarios run columns;
resize covers columns and sidebar. The check's own production verdict skips its
development-server-only MCP app check.

Selected final stress command:

```sh
pnpm test:e2e:stress --mode prod --channel bundled \
  --engine chromium,webkit --layout columns,sidebar \
  --only linking,pairedCatalogue --linkings 2 --pairedCatalogues 3 \
  --evidence /workspace/scratch/893-stress-head \
  --out /workspace/scratch/893-stress-head/result.json
```

**14/14 held** in Chromium and WebKit columns. Each engine completed two linking
steps, three replacement cycles in pairedCatalogue, background work, at-rest,
follower convergence and console assertions. This is selected qualification,
not the full stress plan. The runner uses `options.layouts[0]` at
`conversation-stress.mjs:9923`; the comma-list therefore did not exercise sidebar.

An explicit second command with `--layout sidebar` and otherwise the same flags
failed during initial setup, before any linking/catalogue step:
`openStressConversation` waited 15 seconds for `.workspace-list
[data-session-row=...]`, which the sidebar layout does not expose. Chromium
console passed; the setup failed, and WebKit/sidebar was not reached.
Receipts: `/tmp/893-stress-sidebar-head.log` and
`/workspace/scratch/893-stress-sidebar-head/result.json` (one of two held).
The opener, selectors, layout and stress entry are byte-identical to the base;
a separate base browser run was not performed for this setup failure. Do not
count it as passed or infer complete layout stress coverage.

The current #1282 diff was inspected again: it owns edits to
`verification/desktop/scripts/conversation-stress.mjs`, though it does not edit
`lib/stress-conversation.mjs` or the layout component. No shared harness/UI file
was changed here. Integration sequence: after #1282's audit, give the existing
stress opener the sidebar's actual user navigation and make layout iteration
explicit, then rerun this selected command separately for each layout. Keep its
selection/composer and final-rest assertions. This is reported as a harness
coverage boundary rather than folded into #893 or hidden by a larger deadline.

The prior columns stress attempt failed catalogue replacement because its
fixture relied on principal-wide grant replacement. Explicit receiver revocation
repaired that fixture; final columns held all 14 assertions. Final columns
logs/results/screenshots: `/tmp/893-stress-head.log` and
`/workspace/scratch/893-stress-head/`.

The following completed frontend checks used the unchanged frontend inputs
(the later source repairs are Rust/backend fixtures):

| Check | Observed result |
| --- | --- |
| `pnpm client:test` | 1,092 passed |
| `pnpm test` | 451 files / 5,742 passed, plus github-webhooks 44 passed |
| `pnpm typecheck`, `pnpm build` | Passed; build retained its existing chunk warning |
| `pnpm client:docs:check` | Passed with 50 existing TypeDoc warnings |
| UI type generation/check and frozen install | Passed at pinned `nessa_ui` `a32191c26ee27c2d8159ab6498a0f0530a93e3ea` |
| Architecture, protocol consistency | 108 and 72 passed |
| Format, ESLint, lint rules | Passed; lint rules 12 passed |
| Desktop verification helper tests | 680 passed, two skipped |
| Scoped state docs | Three pages' frontmatter, source paths, source commit existence and local links passed; three Mermaid diagrams rendered and visually inspected |

Raw command receipts remain at `/tmp/893-{client-outer-full,frontend-tests,
typecheck-full,build,client-docs,architecture-final,protocol-final,format-final,
lint-final,lint-rules-final,desktop-helpers}.log` in this saved environment.
The fresh final source suites below supersede earlier backend receipts; no
aggregate frontend pass is inferred from individually passing stages.

Commands ran in the saved Linux cloud environment using Node 24.19, the pinned
UI checkout, bundled Playwright Chromium and WebKit, and task-local official
Debian browser libraries. Rust commands used `CARGO_INCREMENTAL=0` and the shared
`CARGO_TARGET_DIR=/workspace/nessa-agent/target`. Private-storage suites ran under
actual host ownership: the restricted sandbox maps ancestors to UID 65534 and
fails their strict ownership setup before the behavior under test. Assertions
were retained.

The first broad Rust build ran out of disk due to existing incremental caches.
Only inactive incremental build artifacts were removed. An initial focused
owner-phone case used a fixture lacking phone-owner authority; it was corrected
to the existing `Fixture::phone_owner()`. Two existing peer wire goldens were
updated for the required false choice; DeviceRead and OwnerPhone byte shapes
remain unchanged. New composition assertions initially assumed no conversation
directory existed; existing environment-audit setup already creates
`conversations/audit/environments`. They now assert no conversation metadata or
colocated grant file is created by attribution. No existing deadline was raised.

## Deliberate failure and recovery controls

Each mutation below was applied alone, produced a runtime assertion failure
rather than a compile error, then restored with the same test passing. Temporary
mutation diffs and before/broken/restored logs remain in `/tmp/893-probe-*` in the
saved environment. The source tree contains none of these mutations.

| Mutated rule | Regression that failed |
| --- | --- |
| Allow choice on device/owner-phone classes | `same_person_cannot_issue_or_relabel_an_owner_device_class` |
| Replace PAKE choice with constant false | `a_same_person_choice_cannot_be_relabelled_in_either_direction` (authentication outcome) |
| Decode retained binary true as false | `private_choice_is_retained_only_by_gateway_records_and_rejects_truncation` |
| Accept missing peer wire choice as false | `gateway_choice_is_required_and_device_wire_stays_unchanged` |
| Omit credential/principal agreement | `unrelated_credential_or_principal_cannot_supply_a_link` |
| Recompute expiry cause from retry time | `a_retry_after_expiry_and_revoke_keeps_the_first_boundary_and_initiator` |
| Skip stale-link preactivation guard | `failed_unlink_blocks_same_key_activation_and_a_false_choice_inherits_nothing` |
| Omit exact credential from admission join | `attribution_requires_the_exact_granted_credential_and_organization` |
| Ignore consumed-removal receipt | `expiry_removal_is_durable_and_invalid_combined_evidence_has_no_partial_effect` |
| Key receiver grants only by principal | `two_devices_of_one_owner_keep_independent_receiver_grants` |

The first principal-correlation mutant unexpectedly passed because the negative
fixture also had empty grants. The fixture was strengthened to retain valid
grants for each mismatched credential and to keep the empty-grants case separate.
The subsequent principal-only mutation failed and its restore passed.

Four additional controls cover the final composition repairs:

| Mutated rule | Regression that failed |
| --- | --- |
| Bind only when native linking is on | `an_agentless_gateway_without_native_linking_opens_independent_grants_without_metadata` |
| Put the production access file under conversations | The same independent-file regression |
| Decode peer JSON choice as false | `peer_records_keep_both_choices_and_refuse_missing_choice_or_relabelled_updates` |
| Match old cleanup to any credential | `expiry_before_the_first_link_consumes_the_choice_without_replay` (newer link and old absent-before/after receipt) |

Their before/broken/restored receipts are recorded in
`/tmp/893-probes-composition.json`.

Four final controls isolate cancellation command-time validation, exact original
revocation matching, publication provenance, and the strict non-null wire field.
All four assertion-fail and restore/pass receipts are in
`/tmp/893-probes-review.json`; the total at that point was 18 deliberate source mutations.
Two further controls remove the authoritative Human-kind check and exact owner-id
check separately. Both fail
`same_person_requires_the_exact_authoritative_human_and_false_choice_needs_no_person`
and pass after restore
(`/tmp/893-probes-human.json`), bringing the total to **20**.
The terminal-time control also breaks the existing canonical cancellation
validator, demonstrating that attribution and history consume the same owner.

The expiry-before-insertion regression was first run against the missing-receipt
implementation. It failed because cleanup returned `Ok(())` despite a trigger
refusing the required receipt. The repaired implementation must fail that write,
restore it, and retain one removal audit across restart and clock rollback.

Runtime fault tests separately refuse link insertion, link audit, unlink, and
unlink audit with SQLite triggers. They inspect canonical phase/credential and
physical row/audit before retry, then verify original cause/actor, unchanged
grants, and exact credential/receiver after restore. Contradictory combined
evidence is refused before any effect.

## Independent review

An initial foundation review found missing subject-kind export identities and
readonly grant version/shape checks. Both were repaired with focused regressions.
A separate design review confirmed the independent dataset path and native-off
cleanup binding and identified the orphan-backup boundary below.

The first full pairing/domain source review at `d54ece0b6` found two issues:

- Major: the custom-source consumer accepted cancelled records with a different
  revocation time/cause, pre-Active cancellation with a supplied credential, and
  replacement of an independent terminal revocation. Private terminal provenance
  now survives existing record history replay. Both the history validator and
  attribution ask the same record-owned correlation validator; no persisted
  schema or enrollment phase was added.
- Minor: `Option<bool>` decoded explicit wire null as omission for non-peer
  classes. A present choice now deserializes as bool; absence alone uses the field
  default. Public request and reply regressions cover every class.

The reviewer traced domain/API/wire/private-state/registry activation and lock
ordering, using directly linked public-codec/domain probes without Cargo or tree
edits. Their SQL/backup and browser execution exclusions are covered separately.
Three domain contradiction tests and the wire-null test failed on the old source;
complete auth/protocol suites passed after repair. A custom-source effect test
also checks no physical link/audit effects from a contradictory cut containing
another valid invitation.

A fresh identity review then found that an administrative role was being treated
as evidence of a Human owner. The implementation stopped the patch sequence and
repaired the relationship structurally: the existing credential registry now
publishes the actual `Principal` id and kind in its coherent evidence cut. A
single `ConsentIntent` rule validates that exact Human owner before invitation
registration, canonical creation/restore, and link effects. Surface issuers and
namespace administrators are not inferred person mappings. The real admin
Integration route was accepted before this repair and is now refused without an
invitation or revision mutation; ordinary false pairing and Integration or
local-operator revocation remain covered.

The broad Rust run also exposed grant selectors that conflated independent
receiver and principal rows. The shared target selector now resolves the exact
credential's receiver for omitted-receiver updates; the principal-only write
checks receiver absence before mutation or replay. Existing M1 replay and grant
regressions cover this, and an independent SQLite control reproduces the old
wrong-row selection alongside the corrected cases. Fixture reads and backup
constructors now use the independent file. Replacement scenarios explicitly
revoke each receiver, including R2 and R3 between repeated stress cycles; the
`applied` and cursor assertions remain unchanged.

After the structural repair, fresh independent identity and grant/storage
reviewers found **no findings** at `6113c616c` and `a3dd63839`, respectively (the
last commit changes only that stress fixture). The identity review traced Human
provenance, immutable owner versus revoker, terminal history, write ordering,
retries and unchanged capability/class/phone boundaries. The storage review
traced shared gate/transaction selection, receiver-absence guards before replay,
exact credential admission and cleanup, consumed receipts, audit atomicity,
backup fences, readonly format refusal and native-off composition. It also ran
five independent SQLite selector combinations and an attached-store audit
rollback probe, all passing. Neither reviewer edited source or shared test
artifacts. Browser execution, full tests and documentation receipts belong to
the coordinating verification below.

## Failed and unrun boundaries

`pnpm frontend:check` stops at two unchanged Tailwind watcher tests at their
existing 30-second deadlines (508 script tests pass, two cancel, two skip).
Isolated and polling runs fail identically. A detached base worktree at
`19d631ebd34c6ece0062cbd0edbbd5e269d54c1d`, with byte-identical test/CSS inputs,
pinned dependencies and Node, reproduces both failures. This remains a failed
local aggregate gate, not a passing check or a reason to weaken its deadlines.
Other frontend stages were run individually; see receipts above.

The raw broad Rust run also encountered eight process-cleanup failures in
unchanged SDK/server process infrastructure. This VM runs `tail` as PID 1 and
leaves adopted children as zombies. The same unchanged compiled SDK cleanup
regression failed under raw init (exit 101, 4.10s) and passed under a task-local
Linux subreaper (exit 0, 0.09s), with the same assertions and deadlines. Final
Rust and browser commands ran under `/tmp/893-test-init.py`, which only reaps its
own descendants and preserves command status. Live descendants left five
seconds after exit cause failure, not a hidden pass. No repository runner or
#1284 implementation was changed. Raw and corrected receipts are retained in
`/tmp/893-process-{raw,subreaper}.log` and
`/tmp/893-process-environment.json`.

The namespace backup reader still treats metadata/grant facts as absent when
conversation metadata itself is absent. The new path-selection regression covers
an existing metadata store and a distinct independent grant file. Exporting
orphan attribution/audit data without metadata is not implemented by #893.
Grant creation still requires conversation metadata; cleanup does not.

No Windows/macOS native execution, physical phone consent, real devices, live
accounts, or full unrelated stress steps were run here. No desktop choice control
was added. The optional state models retain their historical checkpoints; no
new model-check receipt or full Nessa Docs application validation is claimed.
Mermaid previews and frontmatter/source/link checks were performed for the
scoped documentation.
