# Native WKWebView MCP app navigation probe (#1192)

On 2026-10-28, a debug macOS app bundle built from `a947981c4` ran the real
sandbox proxy (`nessa-sandbox://localhost/proxy.html`) in WKWebView. The probe
sent hostile HTML through the production `appDocument` and `cspPolicy` builders
with an empty declared CSP. Temporary code logged every Tauri `on_navigation`
URL and refused any observed hostile external URL. The temporary code was
removed after measurement. The bundle used the app's `tauri://localhost` pages;
this was not a `tauri dev` webview or Playwright WebKit.

The first run attempted script navigation, meta refresh and a document rewrite
followed by navigation to `http://127.0.0.1:48582/probe-*`. The second attempted
script navigation to `https://example.invalid/probe-https` and
`mailto:probe@example.invalid?subject=probe-mailto`. Each frame loaded the
sandbox proxy and `about:srcdoc`, then reported a CSP violation for its attempted
destination. Script and refresh stayed on `about:srcdoc`. The rewritten document
lost its reporter, and the proxy reported app departure. No hostile external URL
appeared in the native navigation log during either six-second observation. A
clicked link in the host page did reach the same hook as `HandToBrowser` in both
runs, giving a positive control for its external navigation path.

- [HTTP and rewrite screenshot](native-probe.jpg) and [bounded host log](native-host.log).
- [HTTPS and mailto screenshot](native-schemes.jpg) and [bounded host log](native-host-schemes.log).

The bundle was built with `pnpm app:build --stage ci --debug --bundles app` and
ad-hoc signing. It finished bundling, but the wrapper's final verifier exited 1
because it searched for a release `Nessa.app` while this test requested a debug
`Nessa Navigation Probe.app`. The host log confirms `app_bundle=true` and
`profile="debug"`. This measures the listed navigation attempts on this
WKWebView build; it does not cover every new-window, download or renderer path.
