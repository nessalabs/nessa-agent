# Same-person pairing: implementation and verification

Issue: [#893](https://github.com/nessalabs/nessa-agent/issues/893).
Base: `19d631ebd34c6ece0062cbd0edbbd5e269d54c1d`.
Initial source checkpoint: `a3dd63839d52a616560cc70eca470f6a82dbb86f`.
Qualification test checkpoint: `d1fba5c8ae89e8b646545e4f0817324e984a38c3`;
runtime code is unchanged from the initial checkpoint.
Documentation-only commits record these receipts. Subsequent sections pin the
qualification and status-projection source checkpoints separately.

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

The initial `pnpm frontend:check` stopped at two unchanged Tailwind watcher tests at their
existing 30-second deadlines (508 script tests pass, two cancel, two skip).
Isolated and polling runs fail identically. A detached base worktree at
`19d631ebd34c6ece0062cbd0edbbd5e269d54c1d`, with byte-identical test/CSS inputs,
pinned dependencies and Node, reproduces both failures. That initial run remains a failed
receipt; the post-publication readiness repair and new aggregate result are
recorded below. Its deadlines were not weakened.
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

## Post-publication qualification

PR #1285 initially published head `66be9575d282c80f752f1832fb42035fa3c4748f`.
Exact-head run [42687213695](https://github.com/nessalabs/nessa-agent/actions/runs/42687213695)
was inspected directly. Change discovery, workflow validation, gateway-contract, frontend and
release-profile desktop compilation passed; the remaining jobs were still
running or queued when this documentation was prepared. Those receipts apply
to that initial head, not the follow-up head.

### Tailwind fixture race

Task-only copies of the unchanged test logged watcher readiness, the physical
watched set, actual filesystem events, HMR hooks and socket packets. At the
backend touch, `getWatched()` contained `crates/server` but not its nested `src`
or `probe.rs`, and no backend event followed. A diagnostic 500 ms delay allowed
the complete watched set to appear and all original assertions passed (847 ms).
This isolated a fixture readiness race rather than a slow Tailwind compilation
or a backend-triggered full reload. Probe logs are
`/tmp/893-tailwind-{stage,events,watched,delay}-probe.log`.

The repair waits for the **exact file** in Vite's watched set before each edit.
Its 5 ms polling is cancellable by the existing test signal and 30-second
absolute deadline; it does not add a fixed delay, retry an edit, extend a
deadline, synthesize an event or loosen an assertion. Each edit must still reach
the actual watch-event hook and produce its own stylesheet update. Backend
content and dependency-graph exclusion, absence of full reload and production
compiler assertions are retained. Both tests now pass in under one second
(`/tmp/893-tailwind-ready-final.log`). No matching open owner was found, and
#1282 does not change this test file; its CSS hunks remain untouched.

The complete `pnpm frontend:check` then passed at the qualification test tree
(exit 0), including 510 script passes/two skips, 680 desktop helper passes/two
skips, 108 architecture and 72 protocol checks, docs/typechecks, all 451 files /
5,742 frontend tests, 44 webhook tests and the production build. The existing
50 TypeDoc warnings, React test act warnings and build chunk-size warning remain
disclosed; none was changed to pass. Full receipt:
`/tmp/893-frontend-followup.log`. No aggregate pass is inferred from the earlier
individually passing stages: this is a fresh successful end-to-end command.

### Human owner and the actual CLI

The restriction concerns the authenticated **principal**, not the caller label.
`composition/auth_command.rs` bootstraps a Human owner; the default owner output
is `auth/surfaces/nessa-cli.token`. `LocalGateway` labels the session `nessa-cli`
but retains the credential's principal. `nessa call pairing.create` validates
`samePerson` against the product schema and passes it unchanged to the gateway.
A separately provisioned surface is an Integration even if administrative and
issued by the owner; issuance provenance is not a current acts-for relation.

The existing native Human/Integration route regression now uses real
`cli::application::invoke` and `LocalGateway` for the positive request, with a
protected throwaway owner-token file. It passes through the catalog, schema and
product transport, then inspects the retained Human-owned true invitation.
The full native suite passed 139/139; formatting and focused native Clippy
passed. Clippy replayed the dependency's existing long constant-evaluation
warning. Receipts: `/tmp/893-followup-{native,fmt,clippy}.log`.

The API/owner-CLI behavior is implemented. A complete desktop flow remains
unimplemented: define an authoritative Human relation for the Integration
panel, add a creation-only choice control after #1282's UI audit, then qualify
true/false consent and its read/cleanup boundaries through the browser. This
branch does not infer that relation, hand the panel an owner token, or change
#811 phone consent. The scoped state page now makes these boundaries explicit.

### Sidebar stress

Source tracing confirms `sessions-in-sidebar` omits `SessionList`, while the
unchanged opener and selection check require `.workspace-list`. The runner
also picks only the first requested layout. Thus the recorded sidebar failure
precedes every selected feature step. This branch changes neither those files
nor the UI. #1282's current diff and ownership were checked again at
`9bd3e7c2477684b75867fb7249eaa34453376509`; the integration sequence above remains
required. No sidebar pass is claimed.

A read-only independent incremental review of `d1fba5c8a` against initial
published `66be9575d` found no findings. It checked exact-file readiness versus
actual HMR/compiler proof, unchanged deadlines/cancellation, real CLI
catalog/schema/transport versus durable Human invitation identity, and the
Integration negative case's no-mutation guarantee. It independently traced
bootstrap Human/default CLI output versus separately provisioned Integration
principals. It ran diff hygiene only; parent execution receipts, full lifecycle
implementation and browser/platform execution remained outside this narrow
review. The earlier full implementation reviews retain their original scope.

## Desktop investigation and canonical status projection

The [desktop completion handoff](893-desktop-handoff.md) records the independently
reviewed identity/provenance gap and current #1282 ownership at
`9bd3e7c2477684b75867fb7249eaa34453376509`. Its Shared UI ownership is an
integration constraint, not a blanket stop on filenames. The existing
Integration actor doubles as invitation owner throughout admission, staging,
receiver ownership and terminal audit, so substituting another Human is not a
safe lookup patch. Provenance revoke/expiry/recovery semantics for pending
invitations and active links need a contract. Settings creates OwnerPhone;
Add Computer eagerly creates an Environment worker; no PeerRead creation UI
exists. Those flows cannot be silently converted. No UI/harness or provenance
authority was changed by this investigation.

One independent prerequisite was safely available: read back the choice from the
canonical invitation. Source checkpoint
`311ad40ca846a09689e6ccaea29c2cebdcfe758e` adds optional `samePerson` to the existing
owner-status schema/generated types and one existing projection. All create,
read, pending, approve, cancel and deny paths use it. Both peer choices are
explicit; Device and OwnerPhone replies omit it. Legacy absence remains unknown.
The shared TypeScript decoder preserves false and rejects malformed/non-peer
fields without caching a local request choice. The field is historical consent,
not a committed person id, eligibility grant or read-admission fact. No storage,
credential class, policy, epoch, lock or lifecycle transition changed.

Before implementation, the new client regressions failed at runtime because the
choice was dropped and malformed fields ignored; the real route failed
`Null != false`. Logs: `/tmp/893-projection-{client,route}-before.log`.
After repair: full native enrollment **140 passed**, including non-peer omission
and owner-side status after restart; full client **1,094 passed** under host
ownership; final owner-route **16 passed**, including explicit Environment false
and true; focused client **12 passed**. An initial broad client run in the
restricted sandbox failed only the two unchanged private-file setup checks;
the host-owned rerun passed. Those files/assertions were not changed.

The full frontend aggregate passed **451 files / 5,744 tests**, plus **44 webhook
tests** and build, with its existing warnings. Protocol checks (72), client and
root typechecks, SDK docs, formatting and all-target Clippy across the nine
packages passed. Final minor test-only class/Environment coverage refinements
were checked again with owner routes, client tests, focused Clippy and formatting.
Receipts: `/tmp/893-projection-{native,client-host,owner-final,client-final,
protocol,frontend,clippy,native-clippy-final,fmt-final,format-final,sdk-docs}.log`.

A 21st deliberate mutation replaces the optional-field presence check with a
truthiness check. It drops false (and skips malformed falsy values), causing
assertion failures; restoration passes. `/tmp/893-projection-probe.json` and
`/tmp/893-projection-false-{mutated,restored}.log` preserve both outcomes. No mutant
remains. The original 20 lifecycle controls retain their earlier checkpoints.

A fresh read-only incremental protocol review of `987b1c22` → `311ad40c` found no
findings. It checked all status paths, class/choice agreement, legacy absence,
stateless decoding, non-peer omission and the separation of consent from
committed link/admission. It independently ran the focused client suite (12
passed; `/tmp/893-incremental-pairing-review-client.log`), without edits or Cargo.
Rust/browser execution and the broader original implementation remain separate
verification scopes. A separate read-only handoff review corrected two wording
issues: the future sequence now branches consistently between relation and
reauthentication paths, retaining whichever actor actually authenticated; and
removal prose names the actual revoking principal/local operator or expiry,
not only the original invitation owner. No runtime code changed for those
corrections.

Production qualification at source `311ad40c` passed:

| Check | Chromium | WebKit |
| --- | --- | --- |
| resize-observers | pass | pass |
| mcp-apps-gateway | not run: dev server only | not run: dev server only |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |
| parent-stop | pass | pass |

Scripted verdict: **pass**, relevant log lines: none. The same production-mode
command as above used evidence `/workspace/scratch/893-scripted-projection`;
summary: `pr-summary.md`, log: `/tmp/893-scripted-projection.log`. A final WebKit
sidebar fitted screenshot was visually inspected. Selected stress explicitly
used `--layout columns --engine chromium,webkit --only linking,pairedCatalogue
--linkings 2 --pairedCatalogues 3` and held **14/14**, including both linking
attempts, three catalogue cycles, background/at-rest/follower/console checks.
Receipts: `/tmp/893-stress-projection.log` and
`/workspace/scratch/893-stress-projection/result.json`. Browser commands ran
sequentially with Cargo under the same disclosed task-local init/library setup.

No true-choice desktop browser acceptance is claimed: there is no implemented
Human-proof flow or choice control. The earlier sidebar stress setup failure
remains a documented boundary; it was not rerun or changed here. The new scoped
state page checkpoint, source paths and local links were validated; its chart
is unchanged and no new formal-model receipt is claimed.

Current exact-head CI results are recorded in the PR, because publishing a new
documentation receipt would create another head. Prior-head results are never
relabelled as the projection's CI qualification. No merge or deployment occurred.
