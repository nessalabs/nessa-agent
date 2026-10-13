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


## Bounded review handoff

Current product source: `a9ae357fbcd4372e2af264706bc2cf907dc92fa9`. The screenshots above remain captures of `7870e750`; they are not claimed to capture the later fixes.

The final source adds stable listener/cleanup ownership and preserves same-owner closed-tab notices while suppressing redirection into replacement providers. Author checks passed 4,193 tests in 322 files, TypeScript, ESLint, formatting and architecture. Both closed-target regressions fail when the owner guard is removed and pass after restoration. The private current-main assembly `9859c46f` passed 4,326 tests in 333 files, TypeScript and ESLint after integrating this source.

Fresh R5 found no remaining source correctness finding. A suggested comment finding was withdrawn after checking the actual historical wording and host event order. Browser gate 17 remains open: the combined matrix at `c82a58e7` timed out on WebKit's second Add attachment control after replacement-owner selection. The author also reported this full-matrix timeout on Chromium. Later passing 24-case runs and isolated passing scenarios do not explain the original failure. [The preserved combined failure](owner-swap-timeout.log) includes the UI text, no page errors, and the exact failing line.

The proposed cause is not established. Next investigation should capture control names, the committed React owner, and the original/replacement stores at the failing interaction, then establish whether the UI or verification fixture violates its transition contract. Do not weaken the selector, add sleeps, or count a passing rerun as a fix. This stays a draft handoff under the repository's bounded review rule.
