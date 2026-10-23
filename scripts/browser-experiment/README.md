# Dedicated browser experiment · #1089

**Recommendation:** evaluate a dedicated persistent headful Chromium worker, with
human and agent input reaching the same page. Keep its test profile on that worker.
Treat local/cloud portability as reconnecting to the worker, not copying a logged-in
profile between operating systems.

**Current outcome: partial browser qualification.** The first supported-runner
execution passed rendered sandbox status, the toy extension, shared viewer/agent
controls and navigation/download/popup checks. It then exposed a failed-navigation
recovery race. The repair waits for error-document settlement before releasing the
queue; its new regressions pass locally and await another real-browser run.
Persistence/restart, profile separation and performance qualification remain
unverified. This local environment still cannot launch sandboxed Chromium.
Nothing is integrated into Nessa or deployed.
See [RESULTS.md](RESULTS.md) for the actual run and remaining blockers.

## Ownership and scope

This is developer tooling, contained under `scripts/browser-experiment/`. It has an
independent lockfile and does not change the application workspace dependencies,
audit pipeline, webhooks, gateway, Tauri window, or conversation tools.
The isolated qualification workflow lives at
[`../../.github/workflows/browser-experiment.yml`](../../.github/workflows/browser-experiment.yml)
and runs only for pushes to this experimental branch.

```text
runtime/main.mjs       composition: fixed test profiles, fixture and shutdown
runtime/experiment.mjs ordered resource cleanup, including partial startup failure
runtime/cleanup.mjs    combined failure preservation and explicit cleanup recovery
runtime/browser.mjs    Playwright persistent headful context, downloads
runtime/session.mjs    single ordered page command stream, input limits, close fence
runtime/navigation.mjs Chromium loader-identity and DOM barrier after failed navigation
runtime/server.mjs     loopback HTTP transport and capability credential
viewer/               page-frame viewer and pointer/text/keyboard forwarding
fixtures/site.mjs      self-authored loopback site, storage and download fixture
fixtures/extension/    self-authored minimal MV3 extension
tests/                transport/lifecycle tests and real-browser verifier
evidence/             committed results; latest/ is ignored local output
```

Arrows below mean calls, not independent browser instances:

```text
viewer pointer/text ──► authenticated loopback API ──► session queue ──► one page
agent API command ────► authenticated loopback API ──► same queue ────► same page
page pixels ──────────► JPEG response ───────────────► viewer
one persistent context ──► dedicated test profile on worker filesystem
```

The page-frame viewer is a small remote-display experiment, **not noVNC or a full
desktop stream**. It polls sequential JPEG requests with a 250 ms delay after each
response. It excludes browser chrome, extension toolbar popups, OS dialogs and tab
switching. This deliberate limit makes it insufficient for a complete human takeover
product. The local operator can also use the actual headful window on a supported
desktop. No agent-evaluation endpoint or externally reachable CDP port is added.

## Lifecycle and ordering contract

| State/event | Behavior | Evidence |
| --- | --- | --- |
| Launch | Non-root, `headless:false`, `chromiumSandbox:true`; no fallback | Launch-options unit test; real launch blocked |
| Ready, command | Shared page, ordered dispatch; at most 16 pending commands | Queue/close regression |
| Invalid command/input | Typed refusal before browser operation | Input/transport regressions |
| Navigation fails, error document still committing | Hold the command queue until a new root-document loader matching the requested/error URL has its own DOM-ready event; then publish the original typed failure | Controlled commit/load regression, including hash and stale-document events; first CI run exposed this ordering |
| Error-document settlement cannot be confirmed | Publish original and settlement failures, fence further commands; no automatic navigation retry | Settlement-failure regression |
| Close starts | Fence arrivals and queued commands; await active operation | Queue/close regression |
| Context disconnects | Fence further commands | Unit regression; browser scenario not reached |
| Shutdown | Close viewer, session/context, downloads, then fixture | Implemented; successful browser shutdown unverified |
| Failed launch | No viewer exposed; verifier removes its temporary test tree | Observed in browser-results.json |
| Browser close unconfirmed | Retain test profile and write primary/cleanup errors; no automatic relaunch | Real-filesystem cleanup regression with substitute browser failure |
| Viewer startup fails after browser opens | Close browser, then fixture; retain primary and cleanup errors | Startup cleanup regressions |
| Restart | Reuse only experiment profile a or b | Implemented; persistence scenario not reached |

Operations already executing when close begins may finish. Neither a queue
acknowledgement nor an HTTP success is a durable audit event. There is no production
Cedar authorization, durable command journal, multi-tenant isolation or recovery
supervisor in this developer harness. Adding those belongs to a separately reviewed
integration, not the existing mainline audit work.

## Reproduce on a supported local host

Use Node 24 and an unprivileged user on a desktop where Chromium's sandbox works.
Linux needs an X display (or a provisioned Xvfb display), browser system libraries
and permitted sandbox namespaces. Use a supported Playwright platform. Install
dependencies through your normal approved package workflow; do not work around a
download denial. The bundled Chromium is preferred for extension side-loading.

```sh
cd scripts/browser-experiment
npm ci
npx playwright install chromium
npm test
npm run verify
npm start
```

`npm start` prints a local capability URL. Open it in your browser. Click the page
image, use the shared count button, focus the note field and send text through the
viewer. The “Agent: increment” button uses the semantic command through the same
HTTP API. The verifier also calls that API directly as the agent-side surrogate.
No actual Nessa model or conversation is connected.

Stop with Ctrl+C, restart, and reopen the new URL to examine profile `a`. For a
separate profile use `EXPERIMENT_PROFILE=b npm start`. The fixture port is fixed at
17891 to preserve origin storage across manual restarts; a port conflict is an
explicit startup failure. Only one manual harness can own that port at a time.

The browser verifier uses a fresh temporary tree and one live fixture origin
across its restarts. It tests persistent cookie/localStorage and MV3 extension
storage, profile isolation, same-page viewer pointer and agent API controls,
navigation, download bytes, popup context sharing, failed navigation recovery,
profile ownership and disconnection. It captures `evidence/latest/shared-viewer.png`
only after the actual viewer has received a real frame. Exit 0 means checks passed,
1 means a test failed, and 2 means browser startup/capability validation was blocked.
Inspect the error as well as the exit code. No screenshot is supplied for this
environment because it never reached the first rendered frame.
The first runner did generate a screenshot in its
[retained artifact](https://github.com/nessalabs/nessa-agent/actions/runs/39920161218/artifacts/12487056900).
The connector returned an artifact download reference, but downloading its bytes
from this environment was denied by the proxy with `403 Forbidden`; no alternate
download route was attempted and the screenshot has not been inspected here.

The qualification workflow uses a standard `ubuntu-22.04` GitHub-hosted runner,
Node 24 and Xvfb. It installs the locked Playwright/browser pair and ordinary
browser libraries, then runs the same tests with the browser sandbox enabled.
It changes no host security settings and uploads only test evidence, not profiles
or downloads. Unsupported sandbox results remain a failed qualification. Ubuntu
22.04 remains supported through April 2027, although
[runner deprecation has begun](https://github.com/actions/runner-images/issues/14254).
This is a temporary qualification target, not a long-term deployment baseline.
Remote execution and its evidence must be checked separately from local results.

`EXPERIMENT_CHROMIUM=/path/to/chromium` selects an explicit test executable when
diagnosing a provisioned host. It does not change sandbox settings. Do not point
this harness at a personal profile; there is no profile-import option in the CLI.

## Local and cloud boundaries

Profiles and downloads from manual runs live under ignored `.local/`. Profile
directories use mode 0700. Test verifier profiles are removed in its cleanup path;
manual profiles are retained intentionally. To erase a manual test, stop the harness
and remove only that experiment's `.local/` tree. Never delete a profile still owned
by a live browser. The observed environment left defunct Chromium/crashpad processes
under PID 1; profile removal is not proof of complete process reaping.

The HTTP server binds only to `127.0.0.1` on an ephemeral port. The random bearer
capability travels in the opening fragment and then in request headers. The viewer
removes it from browser history and retains it in memory; a reload requires reopening
the capability URL. HTML/JS assets are public loopback assets, while frames, status
and commands require the capability. Host/origin checks, no-store responses, CSP,
body limits are covered by transport tests. The configured connection limit is not
load-tested. Treat the printed
URL as a secret. There is no TLS or shared-user security boundary on the worker.

For cloud evaluation, keep the same profile volume and browser worker identity,
connect through an authenticated private tunnel to its loopback viewer, and keep
CDP private. Profile storage needs encryption, access control, lifecycle ownership,
backup and deletion policy before real credentials are considered. Container/VM
provisioning and cloud networking are **not implemented or tested here**. Do not
publish this HTTP listener, run a root browser, add `--no-sandbox`, use privileged
containers, or disable seccomp to make a failing host pass. The
[Playwright Docker guidance](https://playwright.dev/docs/docker) describes non-root
and sandbox prerequisites; a provisioned host must pass the verifier's
`chrome://sandbox` check. “Host anywhere” is conditional on those prerequisites.

## Current engine comparison

Primary documentation checked 2026-10-23. These are documented capabilities, not
comparative runtime measurements; only the dedicated-worker prototype was attempted.

| Approach | Extensions and storage | Fit and maintenance |
| --- | --- | --- |
| CEF Chrome style | Chrome extension API is supported for Chrome-style windows. Alloy's old extension API was removed; Alloy/offscreen is not equivalent. | Native embedding candidate, with Chromium distribution, updates and platform work owned by us. Do not select Alloy offscreen while promising Chrome extensions. |
| Electron | Persistent sessions and a documented subset of extension APIs; unpacked extensions must be loaded again at startup. Arbitrary Chrome Web Store compatibility is explicitly outside its goals. | Another app runtime and security-update obligation; not a compatibility shortcut. |
| Native webview/Tauri | Windows WebView2 is Chromium-based; macOS WKWebView and Linux WebKitGTK use WebKit. Tauri's browser-extension setting is Windows-only. | Preserve the existing Nessa shell, but do not treat its view as one portable Chrome browser. |
| Dedicated Chromium + remote view | Persistent contexts support unpacked test extensions. Same worker/context can serve agent and human input. | Best first compatibility experiment; incurs browser worker, display transport, session security, resource and update costs. This page-only JPEG prototype is not yet a smooth full-browser viewer. |

Sources: [CEF runtime/style distinction](https://github.com/chromiumembedded/cef/issues/3685),
[Alloy offscreen request closed as not planned](https://github.com/chromiumembedded/cef/issues/3859),
[Electron extension contract](https://www.electronjs.org/docs/latest/api/extensions),
[Tauri engines](https://v2.tauri.app/reference/webview-versions/),
[Tauri extension setting](https://v2.tauri.app/reference/config/#browserextensionsenabled),
[Playwright extension guide](https://playwright.dev/docs/chrome-extensions).

A subsequent full takeover experiment should compare an authenticated
[noVNC](https://github.com/novnc/noVNC) desktop viewer with an authenticated WebRTC
desktop stream, retaining a private Playwright pipe to the same browser. That would
include browser UI and extension popups, and must measure input-to-paint, bandwidth,
CPU/RSS, video, clipboard and keyboard behavior. Neither transport is implemented
or benchmarked by this branch.

## Authentication and extension limitations

Persistent cookies are necessary for continuity; they do not guarantee successful
authentication. [Google's OAuth policy](https://developers.google.com/identity/protocols/oauth2/policies)
prohibits developer-controlled embedded user-agents for authorization. A streamed
Chromium view must not be assumed exempt; use the provider's permitted system-browser
or redirect flow. CAPTCHA, anti-bot controls, MFA, device posture and site restrictions
can require human action or make the workflow unsupported. No challenge bypass is
implemented or tested.

Passkeys can depend on an authenticator or device unavailable to a remote worker.
Cookie encryption, OS keychains and device-bound session mechanisms can prevent
copying an authenticated profile. Browser version, profile schema, origin and OS
differences can also invalidate storage. Test-profile persistence on one host does
not establish cross-host portability. Never copy personal cookies, passwords or
profiles into this harness. Chrome's
[debugging security changes](https://developer.chrome.com/blog/remote-debugging-port)
reinforce the dedicated non-default-profile boundary.

The toy MV3 extension exercises only content scripts, runtime messaging, a service
worker and `chrome.storage.local`. Store installation, signed/paid extensions,
extension actions/popups, native messaging, enterprise policy, browser sync,
WebAuthn, DRM, video/audio and real logins remain untested. Browser downloads are
saved under generated UUIDs; download upload/export UI and malware scanning do not
exist. Input lacks full IME, drag, composition, clipboard and accessibility mapping.

## Versions, notices and documentation review

The independent package lock pins Playwright and playwright-core 1.64.1, resolved
from npm on the run date. They are Apache-2.0; their packages retain LICENSE/NOTICE.
No third-party browser, noVNC or CEF binaries are committed. Browser distributions
carry their own Chromium and third-party license notices; preserve those when
packaging. The inspected system browser was Chromium 151.0.7922.173 on Debian 13,
not the Playwright-bundled build. No bundled-browser download was attempted after
the sandbox failure.

Before wider testing, review Playwright/Chromium security releases, update this
experiment's exact dependency and lockfile together, install the matching bundled
browser, and rerun all checks on each target host. Re-record the browser version,
rendered sandbox status, screenshots and measurements. Pin deployment images by
digest if/when a deployment recipe is added. A fixed dependency is reproducible,
not an unattended security-update plan.

Applied the [Nessa Docs skill](https://github.com/nessalabs/nessa-docs/blob/main/skills/nessa-docs/SKILL.md)
to review documentation impact: no production state page changes are needed because
this branch adds only isolated developer tooling. Its module map, lifecycle table
and limitations are owned here. No unrelated state-atlas sourceRevision was advanced.
The desktop verification skill was read; Chromium/WebKit desktop, gateway and
conversation stress scripts do not exercise this standalone harness and were not run.
