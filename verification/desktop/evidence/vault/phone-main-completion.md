# Final phone/main integration of #1158

Runtime integration: `a79f324f39c9cd58c2082206b531b438d794d438` merges main
`41339b0ba52dbddb2441389c9ebce1e4f7426c9e` (#1159) into the repaired relay/vault/Boat
stack. Release mapping: `a162d7ef7cc7abbe5f714f74737b34421ec15e6b`.
Only the matching worker pin changes between those two source checkpoints.

The integration preserves main's explicit uploaded-file references, lazy delivery
preparation under the operation deadline, and selected Local/remote workspace
ownership. OpenCode now uses the shared hosted configuration projection with the
selected host's explicit scoped MCP relay. The actual previous one-argument call
failed compilation (E0061); the corrected call passes the eight-package strict
all-target Clippy selection. Local provider/open credentials are still omitted.

## Completed checks

- Frontend: 422 files / **5,439 tests**, relay **44/44**, typecheck and production
  build passed. Full frontend formatting passed. Product protocol/generation:
  **61 passed**; focused client projection/wire tests **204 passed**; stress-plan
  and mDNS helpers **9 passed**, by the integration reviewer.
- Native, using the eight-package selection: hosted execution **3**, scoped
  hosted MCP projection **1**, input preparation **20**, GitHub **70**, all passed.
  Vault's integration suite **16 passed**. The initial `--lib` vault-only run
  selected zero tests and is excluded from this vault count. The readiness guard
  failed against the stale worker and passed **1/1** after verified publication.
  The final gateway build passed.
- Current UI source: desktop smoke **33/33**, uploaded input rendering/reopen
  **145/145**, vault setup/unlock **3/3**, GitHub opt-in/failure/disconnect **3/3**,
  Chromium and WebKit. The latter two use the actual rebuilt gateway and private
  fixture credentials. No native WKWebView coverage is claimed.
- Current production scripted browser suite: **pass**; resize **5/5**, gateway
  window **12/12**, conversation scenarios **9/9**. Development-only MCP Apps are
  excluded from that production result; its separate development check passed **16/16**.
- Documentation: **136 pages, zero errors**, 33 renderer warnings about the
  existing `sourceRevision` metadata contract. Only the reviewed channel-workspace
  page advances its checkpoint to the exact release-mapping source. Its allocation
  statechart is unchanged; the new text explains explicit input preparation and
  OpenCode's use of the existing hosted projection. Other state checkpoints do
  not advance. Architecture and module maps retain the owners from main and the
  existing stack; no further module move or dependency boundary was introduced.

## Published worker and real provider

[Publication receipt](phone-worker-release-receipt.json) records lease
`b6e4f48aa694ce08`, image
`bsh_1WtCS3oK4SWwNbdpOnBXzFPq_PwIT1aGCwnJSQHWakU`, exact source archive,
stripped binary checksum/size, clean-image/fresh-clone checks and completed
publication-VM deletion operations. Prior images remain saved. Exact protocol
refusal and retained VM identity are unchanged. An initial incorrectly detached
build timed out before producing the binary; its trap status is not build proof.
The corrected detached build completed and produced the independently checked
binary. Known-path exclusions are not an exhaustive filesystem secret scan;
provider deletion status is not independent secure-erasure attestation.

The current real Boat flow **passed**: encrypted token, no Local VM allocation,
distinct saved VMs, real subscription inference, workspace files/identity across
restart and resume, cold coordinator startup, approved local MCP access,
connection-removal/session-close revocation, cross-VM delegation, and missing-VM
refusal without replacement or Local fallback. [Checks](phone-boat-checks.json),
[cleanup](phone-boat-cleanup.json) and [deletion operations](phone-boat-deletions.json)
are sanitized receipts. All four owned VMs are provider-confirmed absent, with
completed deletion operations. One thread close remains unconfirmed after the
intentional missing-VM case; this is not relabeled a successful close. The flow
does not claim a live Boat uploaded-phone-file product test or a live OpenCode
MCP run; those boundaries have source and native fixture checks above.

## Review and remaining limits

[Integration ownership review](phone-main-integration-review.md),
[OpenCode closing review](opencode-merge-closing-review.md) and
[worker publication review](phone-worker-pin-review.md) have no actionable
findings in their recorded scopes. Earlier security and gap-contract reports
retain their source-specific limits. These are not an external penetration test
or a blanket security approval.

The earlier **26/36 broad desktop** run remains historical at `29af2ef`; it is not
relabeled as a full current-tree pass. Its independent main-equivalent failure
families are tracked in **#1162, #1173, #1178, #1179**. The local macOS startup
fixture issue **#1170** is separate. The audit retention-cap observation **#1185**
remains open; no unconditional local audit-cap guarantee is asserted. Boat
lifetime follow-up **#1174** remains separate. Current-head hosted CI is recorded
on the PR after the final push.

Cloudflare deployment and Free plan are unchanged. Future relay stages remain
disabled without a dev fallback. Webhook transport is not end-to-end encrypted;
the relay sees incoming payloads before storage encryption. Future automatic
PR-event-to-chat/agent dispatch is outside this integration.


## CI base checkpoint

After these source-specific local checks, main advanced to
`cfd025c121e1b32c42af7b4b062d64915a481c18` (#1180: owner-phone review reads and
review-wait policy). A read-only `git merge-tree --write-tree` reports a clean
merge with the release head; none of the 20 changed paths overlaps the 43 lease
fingerprint inputs. The phone review reads remain under the existing Cedar
owner scope and do not add vault or relay authority. Local results above are
not relabeled as execution of this newer base. The final PR CI uses GitHub's
synthetic merge checkout; its exact run/base/result is recorded on the PR before
landing. No further worker publication is required for unchanged lease inputs.
