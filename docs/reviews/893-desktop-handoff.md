# Desktop same-person pairing: integration handoff

The API/CLI and grant lifecycle merged in [#1285](https://github.com/nessalabs/nessa-agent/pull/1285),
merge `99e07ba5862936efcb91a0eda4a920c1e0945f78`. Its historical source checkpoint
is `311ad40ca846a09689e6ccaea29c2cebdcfe758e`; the published source
`39cc72c59fdcd8d680f09a8f11b9224410ea7029` passed all thirteen CI jobs.
The desktop follow-up implements the remaining entry, Human proof and browser
coverage. Its exact source, verification and publication boundaries are in
[completion evidence](893-desktop-completion.md).

## Ownership and integration

#1282 and #1286 merged before this work began. The isolated desktop branch starts
from main `994f3f93117d5f670afb5c74d2cf03cf09056bc1` and preserves their Devices,
Linked devices, style and overview behavior. A fresh open-PR/issue search found
no competing #893 desktop implementation. Sidebar performance (#1294), ambient
rendering (#1291/#1249), placement (#1293), Boat and changed-check work retain
their own owners. This follow-up does not merge or deploy.

## Selected Human proof

The [canonical contract](../design/access-grants.md#desktop-human-reauthentication)
selects explicit action-scoped Human reauthentication through the existing
credential authority. The normal panel remains `surface:nessa-panel`, an
Integration. The person supplies an owner token; a separate, non-reconnecting
client pins the connected product URL and compares gateway, organization and
audience before invitation commands. The Human is the actual owner and audit
actor. No admin, namespace, caller-label or issuance-history inference supplies
person identity. The registry still refuses an Integration token selecting true.

Only a pairing port reaches the private client. No owner file is read, no browser
login substitutes the panel, and no application storage or URL retains the token.
Input clears on submit/pagehide. UI teardown covers navigation, late setup,
terminal observation and a scheduled ten-minute timer. The timer depends on the
event loop; it is not credential expiry or a server TTL. JavaScript string
zeroization is not promised. Fresh recovery requires explicit sign-in.

The earlier proposed durable Integration-to-Human relation is not implemented
and is not a dependency of this selected path. It would need its own canonical
identity, credential, revocation, restoration and audit contract; this follow-up
adds none of those authorities.

## Consent and recovery

Devices → Add device → Gateway creates PeerRead invitations and makes the choice
before a code exists. Phone remains OwnerPhone (#811); Computer remains an
Environment worker. Their adapters, classes and consent do not change.

Created invitations display the server's immutable true/false choice. Legacy
absence stays unknown. Resume reads fresh canonical status, without asking the
choice again or recovering a lost code. A lost create answer is not replayed.
Retryable activation can continue; permanent activation directs cancellation and
new pairing. Terminal observation retires the old scope and code; explicitly
resuming an unfinished terminal receipt retains a fresh scope only for Finish
removal. An uncertain answer points through the always-visible Back action.

Active and a saved choice are historical facts, not proof that attribution
committed or that reads are allowed. The existing grant owner commits links and
audit after credential activation; it owns failed-write retry, revocation,
expiry, enrollment cleanup and stale-credential prevention. Share grants,
receiver subjects, `hold.peer`, PeerRead and DeviceRead remain unchanged.

## Acceptance evidence

The browser check uses actual disposable native gateways and an actual panel
Integration credential. It enters Human proof through the UI, drives both
choices, refreshes/resumes, claims and approves the exact fingerprint, interrupts
and restores the link-audit write, verifies actor/before/after evidence, revokes
through the existing UI, then re-enrolls the same key false without inherited
person attribution. A receiver collision separately drives permanent activation
and cancellation before credential publication. Both engines and both layouts
are required; conversation stress keeps its background, follower, at-rest and
console assertions.

Native admission, expiry/enrollment cleanup, immutable wire binding and fail/
restore regressions retain their merged API checkpoints. Desktop evidence does
not claim a new native lifecycle, a real device pairing, WKWebView execution or
live-account access. Current passed, failed, cancelled and unrun gates belong in
the completion evidence, not an unconditional completion claim here.
