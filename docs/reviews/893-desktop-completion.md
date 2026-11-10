# Issue 893 desktop completion evidence

Current assembled-source/check-harness checkpoint:
`ea66af43aecc404e17b1c481ffcac568eec92649`, integrating main
`a991ba4a7736867e8acfdb8154ba76c07e783305` unchanged.
Phone cancellation and Gateway selected-control focus changed at
`b2db90dd4`; the Gateway immutable-selection controller last changed at
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

Main advanced with #1293's Place refactor and #1301's single placement decision.
Their integrations preserve the owning changes. Independent review found no
pairing, principal, grant, admission, protocol, SDK or frontend change.
`Place::Pool.name()` preserves the environment listing's `peer:any`; channel
and placement wire/storage words are retained. The decision's internal
`Creation::Person` means ordinary client creation, not authenticated Human
provenance. The shared stress edits are disjoint: Gateway pairing and the exact
Devices inventory coexist with upstream's await/fork/channel checks and its
explicit local thread environment. The native
gateway and MCP fixture build passed at prior checkpoint `3cb4eecd`. At that integration the diagram
body was unchanged; the reviewed desktop source checkpoint advanced, while the
native lifecycle and unrelated OwnerPhone checkpoints retain their own scope.

The complete default development functional matrix at `88bc8b58` held
1,530/1,620 attempted cases in 7,798.3 seconds, exit 1. Twenty-seven families
held, ten failed and command-order could not fully run; eighteen cases were
unable to run. The separate performance budget was omitted explicitly. The
production scripted aggregate passes, including native pairing 4/4. Selected
Gateway stress holds 10/10 default and 14/14 raised per layout, in both engines.
The raised cohort uses gatewayPairings3, parallel6, turns4, followers4, rounds40
and seed7. These are execution receipts for that source cut.

Frontend, SDK and functional `run-all` sources were byte-identical between
`88bc8b58` and assembled checkpoint `3cb4eecd`. Those historical browser
observations do not qualify the subsequent phone cancellation or selected-control
focus changes, or establish native Rust behavior at the new main. Conversation stress and its fixture changed upstream
and require fresh integrated execution. Fresh native/browser integration, the separate
development retained-app check, exact-configuration main controls and final-head
CI are recorded on the PR's live evidence comment. Qualification remains open.

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
fixture inconsistencies. The complete matrix remains red on the broader gates
recorded above. Issue #893 is not declared complete while qualification remains
open; passing focused pairing checks does not waive those gates.

## Follow-up cancellation and focus qualification

Published head `8fc4301afed597d998d773453d9d9fed9e3501e3` was checked by
[CI 42856360767](https://github.com/nessalabs/nessa-agent/actions/runs/42856360767).
Its synthetic merge `f57060972f3299c6ed30f61a56c7c10c23ad2e29` has the same tree
(`49373734419afc2507bf051c3eb7f70ef1768249`). macOS passed. The complete gate
failed on browser stress and Windows conversation tests, including one bounded
failed-job rerun; it is not a green qualification receipt.

Attempt one reached phone linking step 059: Cancel left its modal open and did
not reoffer Pair a device. A separate controlled reproduction establishes that
a quiet poll can disable Cancel between pointerdown and pointerup. This is a
real defect in the same code on main and the candidate; the CI screenshot alone
does not prove that timing caused its failure. The correction lets only Cancel
supersede a quiet poll, retains the captured invitation identity, and rejects old
results through the existing sequence owner. Initial/reconciliation reads and
mutations remain protected. Keyed DOM controls prevent a gesture begun on A from
cancelling a later invitation B. No credential, enrollment, grant or consent rule
changes.

Attempt two stopped earlier at native gateway Chromium/sidebar: “a status poll
stole focus”. A held, exact status response reproduces native disabling of the
selected control. ARIA unavailable state and explicit activation guards preserve
focus while the existing operation latch keeps serialization. Each native case
now owns its disposable grantor stack, so a failed case cannot leave an invitation
that invalidates later cases. The true credential → Integration revoke → false
credential sequence still reuses the same peer key within each case to test stale
attribution cleanup. Fixture response holds preserve socket/request/invitation
identity and default transport forwarding.

The phone fixture's default read-only class is DeviceRead
(`gateway-conversation-read`), not PeerRead (`peer-gateway-conversation-read`).
Created phone codes explicitly request OwnerPhone and use its existing
`gateway-owner-conversation-read-write` class. Earlier scratch probe metadata
labelled the default class “scriptedPeerRead”; that label was incorrect. Those
probes do not prove PeerRead or OwnerPhone semantics. The new gesture regressions
exercise actual OwnerPhone requests and preserve their class on cancellation.

The correction's focused unit suites hold 117/117 before integration; upstream
changes no TypeScript source in those suites. Eleven individual unit mutations
all fail their expected regressions, then restore exact file hashes. Restoring
the row's old poll-disabled binding fails only the new row gesture case in both
engines (22/24 hold). Removing the row's invitation key fails the retarget case in
both engines (22/24 hold). Reintroducing native disabled on Gateway Cancel fails
all four engine/layout checks with `nativeDisabled:true`; Chromium also reports
`focused:false`. All three browser mutations restore the exact intended source.
A restored development phone suite holds 24/24, and the previously failing native
Chromium/sidebar case holds 1/1 with an exact held response. These focused receipts
precede the Rust clock/type integration; the final integrated production and
stress results belong to the PR's live qualification record.

The integrated native gateway and MCP build at `ea66af43` passes in 126 seconds,
with one long constant-evaluation warning in the existing protocol macro. Both
owning desktop pages have been fully reconciled with committed source; all six
diagrams render and the documentation validator reports 180 pages, zero issues.
The historical TLA receipt is retained unchanged and does not cover quiet-poll
cancellation or DOM identity. Independent implementation review reports no
findings in cancellation target/sequence ordering, result ownership, reconnect,
Gateway activation guards or fixture consent-class relationships.

Both Windows attempts passed 1,454 tests and failed one, with six ignored. The
first failure was `a_fetch_timed_out_or_cancelled_is_recorded_ended_timed_out_or_cancelled`;
the rerun passed that case and failed the adjacent
`a_fetch_whose_store_stays_busy_still_ends_on_its_wait_or_its_stop`. Each failure
is a task-join watchdog expiry, with different admission and timing evidence:

| Test | Evidence before cancellation | Join bound |
| --- | --- | --- |
| `a_fetch_timed_out_or_cancelled_is_recorded_ended_timed_out_or_cancelled` | Host observes the exact call; frozen-clock finish succeeds | Lesser of remaining wall watchdog and five seconds; remaining budget was not logged |
| `a_fetch_whose_store_stays_busy_still_ends_on_its_wait_or_its_stop` | Twenty milliseconds elapse before stop; no exact admission handshake | Fixed five seconds, without FrozenFetchTime |

Passing main controls do not establish the cause of these intermittent failures. Main
`a991ba4a7` leaves both test bodies, cancellation selection, fetch accounting,
`end_fetch`, SessionManager and storage unchanged; its new sleep wrapper directly
returns the same Tokio sleep.

The unresolved boundary is cancellation versus durable final FetchEnded saving.
Save-entry observation is not commit evidence. The next actionable investigation
is failure-only per-request stage timestamps and save-entry/commit snapshots,
comparing isolated Windows execution with the partition while retaining every
assertion and deadline. [PR #1303](https://github.com/nessalabs/nessa-agent/pull/1303)
owns adjacent publish/fetch wait configuration; this UI follow-up does not change
that owner's Rust paths. No further blind failed-job rerun is claimed.

The final integrated local checks at `ea66af43` pass: frontend 456 files /
5,835 tests, 44 webhook tests and production build; phone production 24/24;
native Gateway production 4/4; phone and Gateway conversation stress each hold
10/10 default and 14/14 raised per layout in Chromium and WebKit. Raised runs use
three repetitions, parallel6, turns4, followers4, rounds40 and seed7. The complete
production scripted aggregate also passes its available families, including a
second native Gateway 4/4; the MCP sandbox family is explicitly dev-only and was
not run in that mode. The [machine-readable local receipt](../../verification/desktop/evidence/gateway-pairing/qualification-ea66.json)
and [aggregate summary](../../verification/desktop/evidence/gateway-pairing/scripted-prod-summary-ea66.md)
retain the commands and boundaries. They do not make final remote CI green.

The long multi-command local session ended without a verdict during the raised
sidebar phone run; no result file or live process remained. Its cause is unknown.
The same raised case subsequently passes 14/14 in an isolated session under the
unchanged bounds. The interrupted attempt is retained separately and is not
counted as a test failure or a completed pass.

Preceding main `bc8cb5109` [CI 42861854503](https://github.com/nessalabs/nessa-agent/actions/runs/42861854503)
passes browser stress and both specific fetch-cancellation cases. Its Windows
conversation partition instead fails
`parent_stop_default_override_grandchildren_and_unrelated_execution` with
Retirement Deadline (1,454 passed, one failed, six ignored). This is a distinct
failure, not a reproduction or diagnosis of the candidate's fetch timeouts.
The later main `a991ba4a7` run had no assigned jobs at observation. Historical
broad functional failures and all 36 selected main control groups are preserved
in [the functional receipt](../../verification/desktop/evidence/gateway-pairing/historical-functional-88bc8b58.json).
That is not a fresh complete matrix for `ea66af43`.

The separately attempted current-main full stress control was cancelled before
its target linking step and has no completed verdict. It has not been restarted.
The denied issue-description update has not been retried.

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
closure review found no issue and closed that P3. Between that full frontend
qualification and `92ef76a3`, no product code changed. Broad browser/stress and exact-head CI remain
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
