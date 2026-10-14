# Attachment gesture owner evidence

Issue #824. Product source was frozen at `7870e750b7483772ebe4ba1a66546660493ef568`,
against #816 source `22f76f61886b05cc598b37ddf3fb3acd9ce124e5`.
The later evidence commit adds these files only.

The actual panel fixture holds two native picker calls across Redux providers
with the same local `c0`, announces their readiness in reverse order, and
asserts the visible provider shows only its own file. It then switches back,
checks the original provider's tile, resolves both results in reverse order,
and checks each store's draft. These cropped screenshots show provider B's
own `b.pdf` tile. They are supplemental visual evidence; the assertions prove
routing and cleanup.

![Provider B file, Chromium](picker-owner-chromium.png)
![Provider B file, WebKit](picker-owner-webkit.png)

[check.log](check.log) records only the short gate and revert results. The full
source ordering table and public regressions live in
[attachment-gestures](../../../../docs/design/attachment-gestures.md).
These controlled host tests do not establish native platform support or server
authorization; current-main assembly and CI remain separate gates.


## Historical review handoff

Current product source: `a9ae357fbcd4372e2af264706bc2cf907dc92fa9`. The screenshots above remain captures of `7870e750`; they are not claimed to capture the later fixes.

The final source adds stable listener/cleanup ownership and preserves same-owner closed-tab notices while suppressing redirection into replacement providers. Author checks passed 4,193 tests in 322 files, TypeScript, ESLint, formatting and architecture. Both closed-target regressions fail when the owner guard is removed and pass after restoration. The private current-main assembly `9859c46f` passed 4,326 tests in 333 files, TypeScript and ESLint after integrating this source.

Fresh R5 found no remaining source correctness finding. A suggested comment finding was withdrawn after checking the actual historical wording and host event order. Browser gate 17 remains open: the combined matrix at `c82a58e7` timed out on WebKit's second Add attachment control after replacement-owner selection. The author also reported this full-matrix timeout on Chromium. Later passing 24-case runs and isolated passing scenarios do not explain the original failure. [The preserved combined failure](owner-swap-timeout.log) includes the UI text, no page errors, and the exact failing line.

The original combined failure is retained above as historical evidence. Follow-up investigation captured the actual control and provider state: the old transition entered the upload-owner fixture, which seeds `replacement.png`; that changes the control from “Add attachment” to “More options.” The helper also did not wait for the replacement provider to commit before the next interaction. The source-asserted reversion recorded in `/tmp/nessa-838-old-upload-fixture-probe.log` restored the seeded upload and failed the new empty-draft assertion, while the dedicated `swapPickerOwner` transition in `b49e780` uses a fresh empty store and waits for its committed owner marker. This closes the historical timeout as a verification-transition defect; it does not treat an isolated passing rerun or a longer wait as proof of a product fix. The current matrix keeps the owner transition explicit and asserts both providers' results.

## R2 regression evidence

The `pathless-host-drop` case delivers prose and an image URL without a batch
beginning, then changes providers while the image fetch is held. The actual App
pastes the prose and attaches the image to the provider captured when the result
arrived. The `browser-picker-fallback` case cancels and reopens the browser input,
changes providers before the delayed `change`, and verifies that the original
provider receives the file. Screenshots show the result in the owning provider:

![Pathless host image in Chromium](pathless-drop-chromium.png)
![Pathless host image in WebKit](pathless-drop-webkit.png)
![Browser picker result in Chromium](browser-picker-chromium.png)
![Browser picker result in WebKit](browser-picker-webkit.png)

The expanded attachment matrix passes 30/30 engine/scenario pairs. The focused
test count, type, lint, architecture, format, fixture-build and source-revert
results are in [r2-check.log](r2-check.log). The screenshots are supplemental;
the browser assertions prove the owner routing and result delivery.
