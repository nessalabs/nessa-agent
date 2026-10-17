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
or explicitly refused. The source implementation is not rewritten. The appended test adapter supplies
fixture window labels to the current three-argument diagnostic; historical
revisions retain their original two-argument call. Dependency
versions are seeded from the source revision's lockfile. The temporary crate is
removed afterwards. The temporary crate denies unused imports; its fixture marks
the load-failure predicate consumed by the real links adapter as used. This
catches an unused re-export that the first fixture version suppressed.
For revisions with the state module, it also compiles and runs `state_main.rs`
as a `harness = false` integration target. That reproduces the macOS test
binary's `cfg(test)` without libtest on Linux. It caught unit-test imports left
unconsumed when test functions are omitted. The state unit tests now belong to
the parent page-load test module; the reusable facts file contains production
code only. This check is about compilation mode, not a native WebKit run.
This tests the real `explain` orchestration and callback
state with a synthetic native boundary, not WebKit or Tauri's real adapter.

The retained production unit tests run in the existing desktop CI suite. The
main-thread native regression also runs there on macOS:

```sh
cargo test -p nessa-app --no-default-features --test page_load
```

That regression explicitly links WebKit through its typed `objc2-web-kit`
bindings, creates a real empty WKWebView, asserts its native `URL` is nil,
and checks the diagnostic state decision plus failure-page and normal-origin
fact controls. It does not drive the complete Tauri application, wait for the
four-second deadline, or load the normal origin from a real server. The assembled native check runs separately:

```sh
cargo clippy -p nessa-app --no-default-features --example page_load_acceptance -- -D warnings
cargo run -p nessa-app --no-default-features --example page_load_acceptance
```

On macOS this example runs the real Tauri/Wry event loop and the production
navigation decision, page-load callbacks, four-second watcher, failure protocol,
startup copy and log writer. Its three incognito WKWebViews use an isolated
fixture configuration: a refused initial `localhost:1498` URL whose native getter
must return nil before the deadline; an allowed `127.0.0.1:1420` HTTP page served
by the owned loopback thread; and an existing failure page with a retained query.
The check waits for the real diagnostic callback after the deadline, then checks
all three native URLs and the configured URL in the temporary host log. It counts
normal and existing-failure starts to catch unwanted replacement. Only external
browser launching is replaced with a recorded production policy decision. The
shipped composition supplies its existing window labels to the same watcher.
The plugin explicitly selects Wry so Tauri's otherwise unconstrained configuration
generic receives its unit default. An independent minimized Rust reproduction
failed with E0282 before that selection and compiled afterward; actual macOS
compilation remains a separate gate.
No gateway, credential, settings, tray or user data startup is invoked. The
fixture's custom identifier, nonpersistent WK stores and temporary log directory
isolate its data. A port collision fails instead of selecting a broader origin.
A bounded watchdog fails a stuck native event loop. The fixture points its icon
at the existing host asset: Tauri's context macro reads a window icon even with
bundling disabled. A Linux probe using the pinned `tauri-codegen` 2.6.3 reproduced
the missing default-icon panic, then generated both dev and packaged contexts
with the corrected fixture path and a macOS target selector. That is context
code generation, not native Rust compilation or WKWebView execution.

The existing macOS `local-auth` job lints and runs this example before the shared
server suite. During that step a standard Rust compiler
[problem matcher](https://github.com/actions/toolkit/blob/main/docs/problem-matchers.md) publishes
compiler message/code/source-span annotations; its registration is removed on
step exit. The executable publishes escaped failure verdicts and the step records
its concise successful observations as check notices. This keeps the owned check's
outcome readable through supported annotations. Other platforms compile its explicit unsupported-platform entry;
they do not qualify WKWebView. This is assembled diagnostic acceptance, separate
from installed/signed-app or physical-device qualification (#562).

Cloud validation (Linux, 2026-10-17): architecture check passed;
`node scripts/check-desktop.mjs` passed formatting and 304 Node tests (3 skipped),
then explicitly skipped native Rust host checks because WebKitGTK is absent.
No Linux result qualifies WKWebView. The native regression and full desktop
Clippy/test/release-profile checks require the existing CI macOS runner.
