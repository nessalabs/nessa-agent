# Desktop same-person pairing: completion dependencies

Inspected source checkpoint: `311ad40ca846a09689e6ccaea29c2cebdcfe758e`.
Initial lifecycle implementation: `a3dd63839d52a616560cc70eca470f6a82dbb86f`;
status projection added in the checkpoint above.
API and ordinary bootstrap-owner CLI support true. Desktop completion is not implemented.

## Current ownership

PR #1282 remains open/draft, branch claude/project-thread-g98ebo, head
9bd3e7c2477684b75867fb7249eaa34453376509. Its inspected diff owns changes in
src/desktop/settings/ui/linked-devices-tab.tsx,
src/desktop/devices/ui/add-computer.tsx, other Devices UI/styles/overview, and
verification/desktop/scripts/conversation-stress.mjs. No owner release or
integration handoff was found. Adapter filenames alone are not blocked, but a
working choice requires changing the actively owned UI and shared harness.
The shared filenames alone are not a prohibition: independent hunks may be
composable after inspection. The primary blocker is the missing authority
contract below. Integrate actual UI/harness changes after merge or an explicit
owner handoff, preserving its semantics rather than duplicating the audit.
Do not contact the owner/external agents from this task.

## Existing flows are different consent classes

- Settings Linked devices uses ENROLLEE = OwnerPhone in
  src/desktop/settings/adapters/linked-devices-gateway.ts:59. Do not add this
  gateway-only flag there or change its enrollment class (#811).
- Devices AddComputer uses PairingEnrollee.Environment in
  src/desktop/devices/adapters/devices-gateway.ts:218. It is a PeerEnvironment
  worker, not a PeerRead reader. Its component eagerly calls gateway.pair() on
  open (add-computer.tsx:173), before any proposed choice could be made.
- No desktop PairingEnrollee.Gateway create call was found. A complete read-peer
  experience needs an actual PeerRead gateway entry; silently converting the
  worker or phone flow changes the very classes #893 must preserve.

## Authority dependency comes before a checkbox

composition/auth_command.rs bootstraps a Human owner. Its default owner output
is auth/surfaces/nessa-cli.token; the real CLI regression proves the caller label
nessa-cli does not change its principal. The panel credential is provisioned as
surface:nessa-panel, PrincipalKind::Integration (registry.rs:provision_surface).
The guide explicitly says there is no owner-token fallback and not to use the
owner token for normal browser sessions (docs/guides/local-auth.md:241,691).
An explicit privileged reauthentication design would itself need a credential-
lifetime/exposure and user-consent contract; silently loading or substituting
the owner secret is not an existing desktop identity seam to wire through.
An admin membership, caller label and SurfaceProvision issuance initiator do not
constitute a current Human acts-for relation.

Current canonical Same person contract validates intent.owner against the
registry's exact Human principal before registration, at commit/restore and in
the effect cut. Changing this to treat Integration as Human would contradict the
contract and regress the falsification controls. Giving the panel the owner token
would change its authenticated subject and is not a safe shortcut.

A follow-up must choose and specify the Human proof path. An explicit Human
reauthentication flow using the existing authentication authority is an
alternative to a durable panel-attribution relation; it needs a contract for
credential exposure/lifetime, actor identity and refresh/recovery. It must not
silently replace the panel credential or use the owner secret as a fallback.
Neither path exists in the current desktop flow. This handoff does not select a
new identity or admission system.

If using a panel-attribution relation, the contract must specify in the existing
credential authority:
1. How a trusted owner explicitly establishes the panel Integration's Human
   attribution, bounded to the exact principal/credential, organization and Human.
2. Its persistence/version compatibility, restore validation, explicit enrollment
   of existing surfaces (no issuer/namespace backfill), revoke/supersession/expiry and recovery
   semantics. A later surface credential must not inherit stale provenance.
3. Separate attributed Human from actual authenticated Integration throughout
   admission, staging, activation, receiver ownership, cancellation and audit.
   How an immutable invitation retains the selected Human and authenticated actor,
   and whether relation revocation invalidates an outstanding invitation and what
   later relation revoke/expiry/recovery does to already-active links. Existing
   owner checks, actor audit, PAKE/public/private retention and replay must agree.
4. Which authoritative fact reaches the existing PairingLinkEvidence cut. Keep
   Cedar as the permission owner, keep gateway grant subjects/classes unchanged,
   and reuse the existing grant owner's link lifecycle. Add no identity/admission
   owner beside the credential registry.

These are proposed requirements, not an implemented relation or a chosen
authentication flow.
The repository has no existing relation to wire through; this is an authority
contract dependency even though backend files are independently editable.

## Concrete integration sequence

1. Resolve and document the chosen Human-proof contract in docs/design/access-grants.md
   and the owning auth contract; implement it in the canonical registry/domain,
   with the actually authenticated actor retained in audit: Integration for an
   attribution relation, Human for explicit reauthentication. Specify the chosen
   proof's lifetime/recovery and refuse absent, wrong-Human, cross-org, expired,
   revoked, superseded and stale-restored evidence.
2. After #1282 merges or explicitly hands off, add a gateway invitation entry and
   creation-only choice in the owning Devices UI. Extend its existing model/port
   to carry the chosen value into pairing.create. Do not create a code before
   the choice; once created, show it read-only. A changed choice cancels/replaces
   the invitation rather than relabelling an existing code. Retain the value on
   refresh/retry without reading it from a mutable toggle.
3. Keep ordinary OwnerPhone and Environment behaviors/classes intact. An optional
   Environment choice must still create an Environment credential and must not
   be presented as read access. If eligibility is displayed, it must come from
   the authority; do not infer it in React from role, namespace or caller id.
4. Add integration/browser coverage for the chosen proof path. For attribution,
   use the real panel Integration credential with canonical provenance. For
   reauthentication, drive the explicit Human sign-in step, verify that actual
   privileged session's lifetime, then restore the ordinary panel session.
   Use disposable gateways/credentials and fake agents; do not inject a Human
   token as the panel's default and claim that exercises Integration provenance.

## Independent status projection work

Before `311ad40c`, only create parameters exposed the choice; owner read/status
replies did not retain it for refresh/restart. An independent review identified
that narrow API gap. A narrow canonical status projection is implemented in
#1285 at `311ad40c` without touching the UI or inventing person provenance. It projects the
saved true AND false choices for peer classes, omits the field for non-peers and
keeps absence on a legacy response unknown rather than inferring false. Future
UI must consume this server fact instead of reconstructing a local toggle.
Native 140/140, client 1094/1094, protocol/typechecks, nine-package all-target
Clippy and the full frontend aggregate (5,744 tests plus 44 webhook tests/build)
pass. Final owner-route checks cover both Environment choices. An independent
incremental review found no findings and independently ran 12 focused client
tests. Browser and publication receipts are recorded in the PR and
[verification evidence](893-same-person-pairing.md).
This prerequisite does not resolve the Human provenance contract or desktop UI.

## Required browser and failure evidence

Use real UI actions for the step under test; clients may set up fixtures/read
outcomes. Both Chromium/WebKit, explicit per-layout invocations (the current
stress entry only uses layouts[0]), default and raised choice counts:

- true and false invitation creation; retain chosen value after refresh/restart;
  an in-transit flipped choice fails authentication, not a UI-only assertion;
- for attribution, a real Integration panel with provenance succeeds; for
  reauthentication, the explicit privileged Human session succeeds and is
  retired/restores the panel correctly; missing/stale/wrong proof refuses
  without invitation, revision or partial grant/link/audit mutation;
- real peer Active precedes attribution; hold/fail link or audit insertion and
  prove no person is admitted until commit; retry reuses credential/receiver;
- no share means no read; grant subjects/capabilities/epoch, hold.peer and
  DeviceRead/PeerRead/OwnerPhone classes are unchanged by attribution;
- UI credential/enrollment revoke, expiry and failed unlink recovery; replacement
  false waits for owed removal and never inherits person; old cleanup does not
  remove a newer exact-credential association;
- preserve #1282 selectors/semantics/hunks and all final at-rest/follower/audit
  assertions. The sidebar opener also needs real tree navigation: it currently
  requires a .workspace-list row that sessions-in-sidebar deliberately omits.

Source regressions, mutation controls and passing local/remote receipts for the
implemented API remain separate from this unimplemented desktop acceptance.
No shared UI, harness or runtime code was edited for this investigation.
