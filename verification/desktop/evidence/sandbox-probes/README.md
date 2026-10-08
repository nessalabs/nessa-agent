# Visible sandbox probes — 8 October 2026

The grid over the desktop sidebar came from `departuresOn` placing test iframe
proxies at fixed top-left coordinates with z-index 9999 on the product document.
That placement entered in `7b98140400`. Behavior checks exercised isolation and
navigation, but neither their placement nor explicit resource release.

Departure and chart checks now use a labelled, same-origin fixture document.
They verify document identity, absence of product markup, DOM and geometric
containment, listener abortion and frame removal. Cleanup runs in `finally`, so
failed behavior checks retain separate cleanup evidence.

The held-image scenario records its unanswered request and observed messages
before release. After answering, it waits for the owning image to decode before
closing the server or removing frames. A missing owner is a failed check, not a
successful release. No console error exemption was added.

Environment: macOS 26.6, Apple M4, Node 26.8.1, bundled Playwright Chromium/WebKit;
headed development server, both desktop layouts. These are functional sandbox
checks, not performance measurements. Production sandbox behavior is unchanged.

`placement-fault.log` restores fixed top-left chart positioning and proves the
containment check rejects it in both Chromium layouts. `release-fault.log`
disables listener abortion and frame removal and proves both are rejected.
The source was restored byte-for-byte after each fault.

Final functional matrix: **28/28 held** (Chromium/WebKit, columns/sidebar).
`browser.json` retains the raw observations and cleanup geometry. The native
`probe-host.png` shows the labelled fixture and live chart. The subsequent
screenshot-only chart run held **5/5** in Chromium, both layouts.

`failure-path.log` injects a chart behavior failure: both chart checks fail,
while both cleanup checks pass. Restored source was then checked again.
Read-only lifecycle review reported no findings after the missing-owner fix.
