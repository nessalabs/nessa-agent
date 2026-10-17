# Missing page URL regression (#916)

The dev-only deadline uses `page_load/state.rs`, whose facts come from the
existing `on_page_load::Started` callback. `links` continues to own navigation
admission. No native URL lookup is needed to decide whether to explain a missing
page. This does not change the failure document, navigation policy, or deadline.

| Published state at the deadline | Outcome |
| --- | --- |
| No event (including a refused initial navigation) | Explain the configured development URL |
| Only `about:blank` | Explain the configured development URL |
| Unserved page observed | Explain that observed URL |
| Failure page started | Preserve that page and its original query |
| App page started, including before a later blank event | Preserve the existing first-load behavior |
| Different window has loaded | Continue to diagnose only the unserved window |
| Navigation to the explanation fails | Log the navigation failure; remain recoverable |

Run the portable reproduction with Cargo and Node, from any directory:

```sh
node verification/desktop/scripts/page-load-regression/run.mjs --base=839b1d50352c833c888254130456ac1355cccfa6
node verification/desktop/scripts/page-load-regression/run.mjs
```

The first command reproduces two nil-URL panics on the original implementation
(4 tests pass, 2 fail). The second passes 9 tests. The runner copies the actual
source and its embedded assets to a temporary crate and appends `probe.rs` in
that module's private scope. `host.rs` substitutes the Tauri boundary: its URL
accessor panics on `None`, like the reported Wry behavior; navigation is recorded
or explicitly refused. The source implementation is not rewritten. Dependency
versions are seeded from the source revision's lockfile. The temporary crate is
removed afterwards. The temporary crate denies unused imports; its fixture marks
the load-failure predicate consumed by the real links adapter as used. This
catches an unused re-export that the first fixture version suppressed.
This tests the real `explain` orchestration and callback
state with a synthetic native boundary, not WebKit or Tauri's real adapter.

The retained production unit tests run in the existing desktop CI suite. The
main-thread native regression also runs there on macOS:

```sh
cargo test -p nessa-app --no-default-features --test page_load
```

That regression creates a real empty WKWebView, asserts its native `URL` is nil,
and checks the diagnostic state decision plus failure-page and normal-origin
fact controls. It does not drive the complete Tauri application, wait for the
four-second deadline, or load the normal origin from a real server. Those native
bootstrap acceptance checks remain separate from the portable evidence.

Cloud validation (Linux, 2026-10-17): architecture check passed;
`node scripts/check-desktop.mjs` passed formatting and 304 Node tests (3 skipped),
then explicitly skipped native Rust host checks because WebKitGTK is absent.
No Linux result qualifies WKWebView. The native regression and full desktop
Clippy/test/release-profile checks require the existing CI macOS runner.
