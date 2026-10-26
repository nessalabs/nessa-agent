# GitHub live events in Settings

Source reviewed: `9500b4b74d3d6a30cf06af1bf41a8a23d4c09c21`. A loopback GitHub provider supplies device sign-in
and App repository selection to a real gateway in an isolated development
namespace. No real GitHub account is authorized and no agent turn is run by
this check. The namespace is removed after shutdown.

`github-live-events.mjs --channel bundled` passed in Chromium and WebKit. The
separate row is below 100 CSS pixels high at 1000px width. Each help popup has
five bullets, stays anchored and within 600px and 1000px windows, and Escape
returns focus while Settings remains open. The switch is 34 × 20 CSS pixels.
The check exercises persistence on reload, failed enable, retry, cleanup-audit
failure preserving sign-in, and simultaneous enable/sign-out leaving zero
repositories. Numeric receipts are in [summary.json](summary.json).

![Live events row](github-live-webkit.png)
![Bulleted help](github-live-help-webkit.png)

The [scripted gateway baseline](scripted-summary.md) also passed both engines.
Real private App-token delivery is covered by provider fixtures, not a real
private GitHub repository. Worker deployment version:
`06df8c42-0291-4e9a-a894-ba48b7947df5` on the existing dev Worker and Free plan.
