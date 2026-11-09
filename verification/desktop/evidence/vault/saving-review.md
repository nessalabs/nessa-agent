# GitHub approval receipt implementation review

Reviewed `/Users/nessa/.codex/worktrees/github-webhooks-1107/nessa-agent`, exact HEAD
`e684e77f20910b8e4c56e920005baa8d9b4d32f0`, against intended CI-repair base
`4c977dad81dc2f2ebd13607dff241c4149427c93`. The explicitly dirty scope was only
`docs/state/services/github-webhooks/README.md` (source reference and pending-receipt
explanation; checkpoint still to be advanced by the coordinator). Review was
read-only; no source, build artifact, cache, credential or deployment mutations.

## Outcome

**No implementation findings at any priority within this repair's reviewed scope.**
The explicit Saving projection corrects the independently reproduced premature
terminal status while preserving the earlier terminal-owner and generation fixes.
This approves the examined code/ownership change, not an unobserved hosted rerun
or a claim that every repository suite is green.

The exact interleaving in hosted run40446076479 remains unproven. Its signed-out
row after approval is consistent with the old source path, and the gated public
regression independently reproduced that invalid transition at4ff387934. The
coordinator's pre-fix log records `SignedOut { last: None, unrecorded: false }`
while the approval audit receipt was held.

## Agreement and adjacent orderings inspected

- `State::Saving` retains immutable generation, user code, verification URI and
  original deadline only. It has no token, AbortHandle or terminal-audit ownership.
  In `complete`, the generation/caller leave the admitted Attempt for the original
  task; operation ID was matched in the shared admission helper. The remaining
  Attempt fields and poll handle drop before receipt awaits. Dropping an
  AbortHandle does not cancel the detached task performing this completion.
- `take_waiting` checks exact operation ID and current generation, takes Attempt
  and installs its next projection under one state mutex. Its temporary SignedOut
  value cannot escape that guard. Denial/expiry retain their actual terminal
  projection; approval chooses Saving rather than exposing a false end.
- Saving participates in `State::generation()`. `status` maps both waiting and
  Saving through `connecting_status`, preserving the same code/URI and original
  deadline calculation. `token` still exposes only generation-matching Connected
  state. The new tests gate both approval-audit and save receipts and assert
  pending status/token withholding until release; the field-preservation mapping
  was also inspected directly. No new wire state or UI polling exception exists.
- The Connecting audit means provider approval before storage, as its port
  documents; public Connected means both audit and verified store receipts. The
  success publication remains after both awaits. Audit refusal publishes the
  existing audit-unavailable end; failed save and rollback retain the existing
  store-unavailable result and attributed cause rather than leaving Saving behind.
- The per-generation mutation gate spans admission, receipts and final publication.
  A same-generation disconnect cannot overtake approval storage; it then deletes
  the resulting sign-in. Cancel can return the already-connected result if the
  earlier completion won the gate. Different-generation restoration remains
  independent of the old gate.
- `retire_stale` clears Saving when access is closed/replaced, but does not abort
  or emit another terminal record for work now owned by completion. The new close/
  reopen tests hold each receipt, queue an old-generation command as a completion
  barrier, restore new credentials, release old work and prove rejection of the
  queued old command, preservation of newer cache/storage and one original
  operation/caller terminal owner. Repeated cleanup cannot duplicate the outcome.
- Production `GithubTokenVault` and `VaultSecret` still carry the captured generation
  through atomic `read_in/write_in/delete_in`, including failed-save rollback.
  The application fixture deliberately models receipt refusal rather than claiming
  its memory store implements crypto fences; the existing real codec/vault tests
  independently exercise stale save/delete admission.
- Product dispatch retains Cedar credential-management admission and locked-vault
  refusal. Unique connection close still clears its vault and invokes GitHub stale
  cleanup independently of audit failure. Settings still polls only Connecting;
  this repair makes that existing consumption rule truthful rather than hiding
  a terminal result in the client. No authorization/deadline/capacity or stage
  decision changed.

Regression source inspected: `approved_sign_in_stays_connecting_until_audit_and_store_receipts`,
`disconnect_during_approved_receipts_waits_and_removes_the_saved_sign_in`,
`a_closed_vault_during_approved_receipts_preserves_a_new_restoration_and_terminal_owner`,
existing audit/save refusal, failed-save rollback, cancellation/late approval,
new-generation restoration and real token-vault scoped-store tests.

## Checks and evidence limits

Independently ran `git diff --check` successfully and inspected actual source,
new/adjacent tests, pre-fix and final test logs, strict Clippy output and browser
result. No Cargo or browser process was run by this reviewer, per the brief.
Coordinator evidence: 67 GitHub/relay/product tests pass in the eight-package CI
selection; strict eight-package all-target Clippy passes; architecture/format and
134-page documentation validation pass. The fresh e684 runtime binary built, and
GitHub live-events browser checks hold3/3 across Chromium/WebKit. These executable
results are attributed to the coordinator. Full default Chromium stress was still
in progress when the report was written; its result and the next hosted run are
not claimed here.

Inspected AGENTS and local order/agreement/audit/evidence rules, design review and
ADR ordering table; the entire GitHub application connection lifecycle, audit/store
port semantics, production token codec and vault-secret generation adapter,
product dispatch/socket-close integration and Settings polling consumption. Source
and test layout remain in the existing GitHub context; no new dependency boundary
or production test switch was introduced.

Excluded unchanged Cloudflare/cryptography internals, a fresh whole-vault security
scan, exact hosted causality, unrelated subscription-capacity/focus/path failures,
Windows/Linux execution on this reviewer, and the coordinator's later docs source
checkpoint. A separate existing documentation observation was sent to the owner:
unchanged storage rows G14/G17/G18 still say “file” where the vault adapter now
owns an encrypted entry (plus an optional legacy file). That terminology predates
this completion repair and is not a runtime finding against Saving; it should not
be read as approval of every unchanged historical storage phrase.

Source SHA256 checkpoints:

- connections.rs: `51f56dc8b0af87bbb97f6db7f94305579b96529937d57b0693bc1a0c16ef24a5`
- tests/github/mod.rs: `9098f4e7283f2af5dd24413909a9fc70b395ae37a9100e1c724df8749b847506`
- ADR1152: `957319e9b59659aae27a5c882d0e185397fb72e7dfa1fa85b98374ff403f8515`
- github-connect.md: `1dcaae6ba2492f484de8a21941805b8c376821bb01fd7d796c09b9830651301a`
- dirty github-webhooks state page: `6aa77d3bf9391c9c8c5220073c576dab6cc589df043b64f603fea9a06fed22f1`

## Coordinator documentation closure

After the code review, the owning design page's file/entry wording was corrected.
It now describes rollback as an attempted deletion and clarifies locked startup,
restoration of durable truth after a stop, and serialized cancel/disconnect during
approval receipts. These are documentation-only corrections; the reviewed runtime
source remains exactly e684e77f20910b8e4c56e920005baa8d9b4d32f0. The affected webhook
state page links the connection lifecycle and checkpoints that source; unrelated
state-page checkpoints are unchanged.
