# Issue 893 desktop completion evidence

Current assembled-source/check-harness checkpoint:
`fe7551adb9ecd1055647b66a2291672144c4ab1c`, integrating main
`f4c1ba3637e7f95361e9fe7009a31a01a8afff94` unchanged.
Gateway-specific runtime code last changed at
`491d9359234aa1218cdcb6d0fde19bfb3719e11f`.
Behavioral/negative-control checkpoint before the layer-only move:
`41cec9e2121cb420c67d8036334840c4832b903c`.
The initial publication integrated main
`26b7263514a98e85a50d3ce3c08a8f6b52f4662b`; its
only changes since the initial base were #1283 README/check-changed tooling.
All 20 upstream changed-check tests passed after integration.
Initial base: main `994f3f93117d5f670afb5c74d2cf03cf09056bc1`.
Isolated branch: `codex/893-desktop-same-person`.

This is the implementation/evidence checkpoint. The follow-up PR's **How we
checked it** section is the live record for broad qualification and exact-head
CI. A passed focused check below does not imply every merge gate passed.

## Current-main qualification reconciliation

Merged #1291 supplies the ambient renderer and explicit elapsed clock. It is
preserved unchanged. The new clock does not control pairing's injected `after`
timer, and no pairing state, authority or consent class changed. Independent
integration review at `e543aa44` found no issue in that relationship, endpoint
agreement, Human-session teardown or immutable selection.

The earlier green CI associated with head `8d9daab9` checked synthetic merge
`28f9fb2b79564754bf23e895ec08b58aeb37c346`, already combining it with main
`f4c1ba36`. Its tree equals the local integration commit `e543aa44`. The earlier
local broad matrix and main controls predated that integration. Their failures
remain historical evidence; they do not qualify the newly assembled checkout.

Two mandatory verification prerequisites contradicted their current owners.
Smoke demanded a subtitle although the Settings catalogue, masthead and UI kit
explicitly require none. It now requires one exact title, catalogue-owned tabs
and no residual masthead copy. The linked-device fixture omitted the read-only
MCP inventory which Connections now reads first. It now advertises and answers
only `mcpServers.list` with the published result shape; unknown-method rejection
and all pairing/consent assertions remain intact. Fresh review found no issue.

Before correction, production smoke held 28/32 and linked devices 2/18 across
both engines. Fixed and restored runs hold 32/32 and 18/18. Inserting an unmarked
masthead paragraph causes four exact extra-copy failures. Removing only the MCP
whitelist entry causes sixteen exact unexpected-method failures. Both source
mutations were restored. The desktop verification unit suite passes 682 tests,
with two skips, and scoped format/lint checks pass. These controls close the two
fixture inconsistencies; the complete browser matrix and new-head CI still need
their own verdicts on the frozen assembled source. Issue #893 is not declared
complete while that qualification remains open.

## Merged prerequisite and ownership

[#1285](https://github.com/nessalabs/nessa-agent/pull/1285) merged at
`99e07ba5862936efcb91a0eda4a920c1e0945f78`. Its published API/CLI source
`39cc72c59fdcd8d680f09a8f11b9224410ea7029` passed all thirteen jobs in
[CI 42698189006](https://github.com/nessalabs/nessa-agent/actions/runs/42698189006).
That PR supplies the canonical grant/principal foundation, immutable choice,
activation/link ordering, persistence/restore, audit and revoke/expiry/enrollment
cleanup. Its native regression and fail/restore checkpoints remain in
[the API evidence](893-same-person-pairing.md).

#1282 and #1286 merged before this desktop follow-up. Repeated issue/open-PR
checks found no competing #893 implementation. The active sidebar, ambient,
placement, Boat and CI work retains its owners. No other owner's branch or hunks
are overwritten. This follow-up does not merge or deploy.

## Delivered behavior and authority

Devices → Add device → Gateway creates PeerRead invitations. The person makes
the same-person choice before a code exists. The server's saved true/false value
owns every later label; legacy absence stays unknown. Resume reads fresh status,
without recovering a code or asking the immutable choice again.

Explicit Human reauthentication uses the existing credential authority. The
normal panel stays an Integration. A separate pairing-only client pins its
connected endpoint, preserves product route/query, translates only exact
`/browser/session` to `/session`, checks gateway/organization/audience, and
uses one setup attempt with reconnect disabled. The registry decides Human
eligibility. The actual Human is invitation owner and audit actor. An admin
Integration token is refused before an invitation registers. No owner-file
fallback, browser-login substitution or durable panel-to-Human relation is added.

The supplied credential keeps its existing authority, while only the pairing
port reaches its separate client. Input clears on submit/pagehide. Navigation,
completion, terminal observation, End and the scheduled ten-minute UI timer
retire that client. The timer depends on the event loop; it is not credential
expiry or a server TTL. Closing does not promise JavaScript string zeroization.
Abort detaches pending shared waits promptly and closes a late owned client.

One immutable selected value holds canonical status, code, exact approval result
and keyed fingerprint. Clearing/replacing selection drops its facts together.
Retryable activation offers retry; permanent activation offers cancellation.
Terminal observations discard the old code/notice and retire the old scope.
Only explicitly resuming unfinished removal keeps a fresh scope for Finish
removal. An uncertain reply points through Back/reopen, and no mutation replays.
Focus follows meaningful view changes; same-invitation polls preserve the
person's chosen control.

Same-person attribution adds no capability. Active and a saved choice do not
prove committed attribution or a share. The existing grant owner and admission
remain authoritative. Grant subjects, receiver identity, `hold.peer`, PeerRead,
DeviceRead, Environment and OwnerPhone consent (#811) remain with their existing
owners. Native enrollment uses the displayed POSIX-shell command on the other
gateway; the owner approves its exact claimed key/fingerprint.

## Focused verification

| Check | Result / boundary |
| --- | --- |
| Related client, composition and Devices suites | 124 passed across 11 files |
| Stress plan and actual tree navigation helpers | 28 passed |
| Typecheck, scoped ESLint, format and diff whitespace | Passed |
| Complete frontend at `491d9359` | 454 files / 5,793 tests, 44 webhook tests, architecture/protocol/docs/type checks and production build passed |
| Deliberate unit mutation controls | 45 intended assertion failures at the behavioral checkpoint; restored focused suite 54 passed; unchanged reducer relocation verified |
| Dedicated production native browser | 4/4, Chromium/WebKit × columns/sidebar |
| Browser errors | 0 in each native case |
| Command geometry | width/scrollWidth 580/580; constrained surface 240/240 in all four cases |
| State documentation | 180 documents, zero structural issues; three Mermaid diagrams rendered and visually inspected |

The behavioral focused runs used source changes subsequently committed unchanged
as `41cec9e2`. The reducer/types then moved unchanged from the model into the UI
layer to satisfy the architecture gate; focused, type and architecture checks
were rerun on the implementation checkpoint. The exact body hash is recorded in
the relocation artifact below. The artifact metadata distinguishes that
mapping from the Git HEAD at execution. Independent round 4 ran at `41cec9e2`; the layer correction has a separate
final review. Broad frontend/browser/stress and remote CI are reported
separately on the PR.

The native browser uses disposable local gateways, fake agents, actual panel
Integration credentials and Human tokens entered through the UI. Per case it
creates five true and two false invitations, refuses an Integration proof,
refreshes/resumes, and drives native claim/approval. A fixture-only audit trigger
leaves Active with no person link/audit, then removal of the trigger permits retry
with the same credential and receiver. The committed addition audit names the
Human and exact before/after. There are zero implicit share rows.

Existing UI credential revoke removes that link with the actual Integration
revoker in its audit. The same native key's later false credential has no inherited
person link. A polled terminal receipt closes the old Human session and removes
its code. The normal panel remains connected.

A fixture-only receiver insert collision exercises permanent activation. Its
transaction rolls back, leaving a reserved unpublished credential, no receiver,
and no person link or false link-audit receipt. End sign-in followed by fixture
owner cleanup permits a different invitation on the same page; it inherits no
old stop. A second permanent stop is cancelled through the UI and its canonical
terminal receipt names the Human. A one-shot private status-send fault separately
drives Back/reopen/resume/cancel of the same invitation without code recovery.

Playwright does not consistently report websocket-close for a destroyed page's
old sockets, including the normal panel's. The browser checks current-document
completion/terminal sockets; unit regressions separately cover pagehide,
unmount, deadline, pending setup and late results.

Artifacts:

- [Unchanged reducer relocation](../../verification/desktop/evidence/gateway-pairing/reducer-relocation.json)
- [Native browser measurements](../../verification/desktop/evidence/gateway-pairing/native-browser.json)
- [Every unit mutation and exact changed rule](../../verification/desktop/evidence/gateway-pairing/mutation-controls.json)
- [Structural browser revert/restore](../../verification/desktop/evidence/gateway-pairing/structural-browser-probe.json)
- [Wrapping browser revert/restore](../../verification/desktop/evidence/gateway-pairing/wrap-browser-probe.json)
- [Changed surfaces](../../verification/desktop/evidence/gateway-pairing/)

The first wrapping measurement did not distinguish nowrap at the default width;
it was strengthened with the 240px surface. The same nowrap change then failed
the intended overflow assertion and the restored code passed all four cases.
The first attempted permanent fault used grant-store reconciliation, which the
current adapter reports retryably; the test was corrected to the receiver's
canonical permanent classification. Backend recurrence policy was not changed.

## Independent review and structural closure

| Round / source | Findings and closure |
| --- | --- |
| 1 / `665e53f0` | Two P2: terminal poll retained scope/code/poll; abort waited for shared work. Fixed in `82e8b16f`. Independent auth review found no additional issue. |
| 2 / `82e8b16f` | P2: permanent stop became retry; P3: recovery named an absent button. Typed protocol result and Back/reopen guidance in `a863f487`. |
| 3 / `a863f487` | P2: separate stop state survived End/deadline/pagehide. Three failing regressions established the common ownership problem. |
| 4 / `41cec9e2` | Fresh review after the structural rewrite found nothing at any priority; independently passed 54 focused and 28 harness tests. Review loop closed. |

The full frontend gate at `41cec9e2` then caught a Devices-layer architecture
violation: a model imported SDK types. `491d9359` moves that pure wire projection
and its tests into the UI layer, retaining the command formatter in the model.
The reducer/type body hash matches exactly; no copied wire contract or exception
to the architecture rule was introduced. The checker and all 108 architecture
tests pass after the correction. Placement review independently passed 54 focused and 28 harness tests and the
architecture checker, with one P3 about raw verification selectors. `92ef76a3`
centralizes those same-output strings, retaining hidden-checkbox coverage. Fresh
closure review found no issue and closed that P3. No product code changed after
the full frontend qualification. Broad browser/stress and exact-head CI remain
the PR’s live qualification record.

The design/order table was updated before each structural change. The final
correction removes independent selected-fact bookkeeping instead of adding more
exit-specific resets. Review covered auth, endpoint agreement, late teardown,
selection/fingerprint/focus, command encoding, composition, native contracts and
browser/stress additions. No reviewer edited source or performed external actions.

## Broad qualification and exclusions

The earlier complete frontend run at `82e8b16f` passed 453 files / 5,780 tests,
44 webhook tests and the production build. It is historical, not final-head
qualification. Subsequent obsolete aggregate/browser runs were deliberately
stopped when review found a defect; they are not passing receipts. The final combined source's broad verdicts and exact remote head are recorded on the PR.

A clean `994f3f93` **dev-mode** smoke baseline was 26/32: Settings' missing
Advanced subtitle in all four engine/layout pairs, and overview keyboard focus
in both WebKit layouts. That mode is stated explicitly; a production comparison
is required before attributing production-gate failures to the baseline. No
assertion or deadline is weakened. Other owners' sidebar/ambient work is not
absorbed into this feature to hide a red gate.

No real device, deployed gateway, live-account grant, paid provider or Boat VM
was used. Real WKWebView/native-host execution remains unrun. No formal model
was authored/refreshed and no TLC result is claimed. The native same-person and
unrelated OwnerPhone checkpoints retain their own review scope. This change
makes no frame-budget claim; its new UI is checked by functional, focus and
geometry assertions.

## Publication boundary

Automatic approval review denied the attempted issue #893 status-body update:
it considered that external commitment unauthorized by the latest user request
about merging #1283. The update did not execute. It was not retried through a
different tool. Project-board lookup was separately unavailable (`Resource not
accessible by integration`). The issue/board remain unchanged. Branch push and a
new draft PR are distinct publication actions requested by the user; their actual
outcome and exact remote SHA belong in the final report. The merged #1285 is not
republished or treated as a draft.
