# Stress recovery and current permission choices

Issue: #1187. Related focus fixture: #1162. Status: implementation in progress.

The user authorized fixing the remaining stress and focus failures while leaving
Nessa PR1174 and iOS PR25 unmerged. Verification must exercise the current product
contract without broad console exemptions or longer functional budgets.

## Owners and evidence

The gateway owns vault sealing across restart. The simulated native app and its
visible unlock dialog restore the test's disposable credential lease; a test
must not bypass that transition by changing gateway admission. The native-linking
stress step owns completion of its intentional restart before another UI step.
The shared `unlockRequestedVault` helper checks backend locked state only after
a window has requested unlock. A listener restart alone does not request that
UI: a first prototype calling the former helper from `switchLinking` timed out
waiting for an absent dialog (pool/channel4/6). That prototype was removed.
The protected-use trigger and dialog lifecycle must be reproduced before
choosing the recovery point; ordinary config editing must not require a token.

The minimized diagnostic now shows a locked vault and no dialog on entering
Accounts. Its sibling GitHub row requests unlock to read protected account
status (`GithubRow.read` calls the existing unlock owner). The dialog arrives
before the ordinary settings click and intercepts it. Recovery belongs after
that Accounts intent, through the shared `unlockRequestedVault` helper, not in
linking and not in the runtime-settings API. The API still uses config.json and
does not request vault access.

The permission card owns available choices and their provider-specific labels.
The focus verifier checks that a held or double answer affects exactly one
request; it must select an offered semantic answer rather than invent a caption.
The original failures and separate diagnostic observations remain preserved.

| Trigger | Required ordering / result | Enforcer and verification |
| --- | --- | --- |
| A pool/listener restart precedes channel settings | Open Accounts, allow its protected GitHub read to request unlock, answer the real dialog, then edit ordinary settings | Shared `unlockRequestedVault` plus pool-before-channel browser regression; the settings API's admission remains unchanged |
| A pending permission offers a provider-specific permanent allow label | Select one offered permanent-allow button on that card; the double click answers exactly one request | `answerOnceOnCard`, semantic `data-answer`; current provider-choice browser regression |
| A held Allow Once returns focus to a composer | Repeated keydown cannot send the preserved draft or answer another request | Existing focus verifier keeps its draft and request-count assertions |
| WebKit on macOS tabs across GitHub row actions | Use the shared engine-specific control-navigation chord, then require focus to move from Disconnect to Choose repositories and back; settled focus must reveal actions | `keys.nextControl` owns the keyboard mode, and the GitHub stress step keeps its two active-element and visibility assertions |
| The stress run omits the optional evidence directory | Onboarding still verifies navigation and host handoff; only its screenshots are omitted | Guard the four onboarding screenshots with the evidence option; run the onboarding step without `--evidence` |
| A tunnel survives a gateway restart | Retained tunnel address/process and a new authenticated browser workspace read must both hold before its restart console lines are attributed | Keep the tunnel restart's provider and browser checks inside `restartConsole.during`, return their verified result, and leave failed or unrelated lines in the console oracle |
| A restart produces repeated identical console text | No old or unrelated failure may become harmless merely because a later connection recovers | Existing observer tests remain authoritative until an event-correlated reproduction proves a correction |
| Overview completion exceeds its existing two-second budget | Keep the failure and measure mounted rows/paint progression before changing rendering | Original broad receipt plus separate mutation-timeline diagnostic; no timeout increase |

The iOS CI screenshot at channel007 shows the unlock modal; the cleanup click
timeout masked that earlier state. A separate focus diagnostic records the exact
pending card's offered labels and absence of the legacy literal. These are test
integration defects, not authorization changes. No worker protocol, VM image,
credential store or application permission policy changes are intended.

## Restart trace

The full WebKit run without `--evidence` reproduced 51/52: all functional
steps held, but two identical connection errors remained. Its temporary
event trace shows the tunnel's internal restart had a browser socket error,
the matching console line, a replacement socket, and a successful browser
workspace read. That restart's callback returned `undefined`, so the strict
observer correctly refused to reclassify its line. The next verified restart
encountered the identical earlier line and correctly refused to reclassify
either copy. The observer and its error patterns remain unchanged. The tunnel
step now returns success only after its retained provider state and browser
read hold. A focused tunnel run passed 5/5 in each browser engine with the
strict console result. Full-run regression verification remains in progress.
