# Product command/configuration review — round 2

Result: **not clean**: one major and one minor finding. No authority bypass was confirmed. This review does not qualify recipient delivery or issue #896 completion.

## Frozen source and scope

Reviewed clean checkout `/workspace/nessa-webhook-recovery`, HEAD `eba83a5fc166c081e33c9bd6df740268d82c63fd`, source checkpoint `6f6bdde6a9fcce9fd6e4fee6ff864b2651b5849c`, against published comparison base `15e211372d4a4d0324415fa3942593fc50e2a3fb`. Main `d3dbf580` is preserved. No source files or shared build artifacts were changed beyond normal execution of three narrowly selected existing tests. No broad suite, CI rerun, publication or external service action was performed.

Read AGENTS, the coding standards/review gate and relevant architecture/DI ownership guidance. Inspected the complete new runtime, private configuration and local-auth composition, product admin/share state and routing, retained registration transition result, bounded intent enumeration, host shutdown integration/report, new fixtures and ordering/state documentation. Followed existing native Auth issuance/revocation/access hold and original receipt paths, registration stop/persistence/publication, application share/unshare and native transactional grant/Cedar decisions. Scope is the product/configuration and credential/registered-principal grant ownership slice. Canonical source/queue/sender mounting and their recovery/recipient lifecycle remain explicitly absent.

## Findings

### M1 — Unshare classification asks the wrong Cedar action and blocks revocation on deleted conversations

Location: `crates/nessa-server/src/product/read_access.rs:704–712`, especially line 708.

Reachable trigger: an owner has a canonical grant on a conversation; the conversation is tombstoned; a webhook runtime is present (including one opened from an existing private outbox without current seeds); the owner calls `conversation.unshare`. Before choosing either the webhook or ordinary target path, the new classification calls `OwnerAccess::admit`, which asks `AccessAction::Act`.

The existing contract deliberately differs: `ShareConversation::unshare` and the native grant transaction use `AccessAction::Unshare`, allowing an owner to remove access on a deleted conversation. `ownership.cedar`'s `owner.act` requires `!resource.deleted`; `shares.cedar`'s `owner.unshare` requires ownership alone. Therefore the new outer check returns Deleted before either actual revoke operation can run. This affects ordinary paired or previously paired targets as well as webhook targets because classification precedes target selection. It is an availability/cleanup and preserved-behavior regression, not an authorization widening.

Expected: use the existing owner/Cedar admission for the action being performed—`OwnerAccess::admit_to(AccessAction::Unshare, ...)`—before target classification. Keep the ordinary native unshare path and the registered-principal retained owner. No new policy or deleted-state condition should be introduced.

Independent corroboration at this frozen head: `owner_act_admits_the_owner_until_the_conversation_is_deleted`, `owner_unshare_and_shares_need_ownership_alone`, and `a_share_change_is_admitted_by_the_access_policy_in_its_transaction` each pass 1/1. The last actually tombstones a conversation and successfully revokes its canonical grant. Logs are `/tmp/nessa-896-runtime-review-2-{owner-act,owner-unshare,native-revoke}.log`. The newly added public product fixture has only live conversations, so its passes do not exercise this new outer regression.

Useful existing fixture API: `ConversationRepository::record_deletion(&id, ConversationDeletion::new(...))`; see `tests/conversation/store.rs:268` for the domain constructor and `:4573–4635` for canonical grant, tombstone and successful revoke. A public product regression should cover both a registered webhook grant and an ordinary/previously paired canonical grant while the runtime is present. Taking access away must remain possible even if the ordinary receiver is no longer paired. Inspect the actual row removal/receipt, not just completion.

### m1 — New target classification precedes existing action-attribution validation

Locations: `product/read_access.rs:617–631` and `:702–712`.

`ConversationCaller::actor()` is the existing owner of action/surface attribution validation, including blank, oversized and control-bearing action IDs (`conversation/application/service.rs:234–255`). The public `requestId` becomes that action ID. Existing application share/unshare call `caller.actor()` before conversation/target lookup. The new outer classification blocks construct a caller but do not ask that validation owner before loading the conversation and reading target Auth origin.

A malformed action ID with an absent or deleted conversation can therefore return NotFound/Deleted before the prior InvalidInput; with a valid owned conversation it can reach target-origin lookup before the inner operation finally refuses its action attribution. The generated parameter structs deserialize requestId as a plain String; the socket's generic request-envelope check validates the separate frame `id`, not this action ID. The inner operation still validates before mutation, so no invalid grant/audit write or authority bypass is claimed.

Expected: reuse `caller.actor()` before each new classification boundary, preserving the existing application validation rule and refusal order. Do not duplicate its string predicates. Add a malformed-attribution control that distinguishes this earlier boundary from the later inner validation.

## Prior findings and ownership checks

- Prior M1 host omission is corrected: the production cleanup function obtains the runtime from ProductRouteState, polls its shutdown first to seal admission, polls other drains concurrently and records its separate typed result. Unknown/failure/deadline evidence prevents confirmed cleanup. Neither an Arc nor another owner's success stands in for this drain.
- The first-panic ordering is corrected: failed/sealed state is set before the oneshot response wakes its observer. The custom waker inspects that exact ordering, rather than assuming task reaping has already run.
- `retain_grant` preserves the original application result inside the existing owner. Registered webhook share/unshare futures retain their native operations and dependencies through completion and are joined by the same job set. The new physical-write probe covers both native mutation cases; no second grant implementation or receiver scope was added.
- Capacity is acquired before spawn; ordinary and control lanes remain distinct. Finished-task reaping prevents a succession of completed jobs being retained. The retained cleanup task and join handle remain in shared owner state across shutdown-observer cancellation; no new guard is held across an application await except the intended async ownership/authority guards.
- W1 remains one retained issuance gate across ordinary and configured modes. Registration replay consumes original actual metadata/transitions and returns no bearer. Bounded intent lookup uses the existing checked row decoder; changed commands refuse before reservation/issuance. Startup reuses original private inputs and actual Auth; a pending issued intent survives caller loss and cold restoration.
- Existing `run_checked` performs owed publication before intent/registration reads, so the previously rejected simple publication-failure/false-cleanup hypothesis is not revived. Ordinary revoke's no-registration stop is an intentional acknowledged no-op.
- Prior three Clippy findings and builder documentation/FQ type nits are closed in the inspected source. No additional production lock cycle or policy bypass was confirmed in this pass.

## Evidence and limits

Independently executed only the three targeted existing policy/native tests above: each passed 1/1 on Linux. Read author receipts for 235 webhook tests (three unchanged retention cases excluded), 6 real configuration tests, 5 report tests, 27 host tests, seven-package all-target Clippy, formatting, 108 architecture tests and architecture check. Read original/corrected panic and physical grant-write failures and the six caught/restored one-rule controls. These receipts support this scope and retain their original source boundaries; they do not prove the missing deleted-resource/action-attribution cases.

Read the two state pages and their exact source checkpoints, unchanged inner diagrams and explicit remaining gaps. No new browser/diagram render, platform qualification or source/sender/recipient test was performed. Published locking CI success is a separate source qualification. Final assembled whole-feature review, actual isolated signed recipient delivery/retry, expiry/revoke races, and the future delivery owner's startup/shutdown/known-outcome coverage remain required.

The frozen checkout remains clean and unchanged. A fresh full round is required after correcting all findings and adding their evidence.
