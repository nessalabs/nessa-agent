# Resource-read evidence

The request classifier remains in `scripts/lib/browser.mjs`; page line retention
and reporting remain in `scripts/lib/page-lines.mjs`. A byte count or another
live app does not identify a completed resource read (#996).

The dev gateway app check observes three existing factories through harness-only
module export decorators. Their original implementations, arguments, return
promises, deadlines and byte stream remain authoritative. There is no production
observer API, application global, second fetch, stream tee or reread. The
decorators are opt-in for this check; a missing or changed factory export fails
closed rather than guessing a source patch. Production checks keep their current
behavior. Evidence names the decorators instead of claiming uninstrumented code.

| Boundary | Evidence owned there |
| --- | --- |
| `createAppBridge` | actual server-port address and mount AbortSignal; original view callback publishes this bridge's live state |
| `fetchMcpResource` | original fetch, response reader, consumed chunks, EOF, reader cancellation and inner request cancellation |
| `createMcpAppsApi` | expected description and caller signal; successful original `fetchResource` promise proves wrapper and production length/digest acceptance |

Opaque per-mount/read identities stay in the harness. The exact single-use ticket
joins the fetch event to one Playwright Request in memory, with no network header
change; a second candidate is ambiguity, never URL-based attribution. Tickets,
raw bodies and session headers are not written to results. Chunks are hashed
incrementally and discarded. The original assembled byte-array identity must
reach the successful client method unchanged.

| State / event | Required next state or refusal |
| --- | --- |
| begin | exact original mount signal, expected metadata and unique read identity |
| reading / chunk | bounded actual bytes; keep reading |
| reading / EOF | exact expected length and SHA256; otherwise failed |
| EOF / original client method resolves | matching byte-array/read identity; wrapper and integrity accepted |
| verified / original bridge view live | exact original mount, still uncanceled; eligible |
| any / cancel, replacement, rejection, duplicate or impossible ordering | permanently failed; later success cannot repair it |
| event / Request binding arrives in either order | join exactly once by private single-use identity; ambiguous candidates fail |
| failed request before or after eligibility | retain its exact Request and line until final page reporting |
| eligibility revoked before reporting | final line is an error, including cancellation after live |
| page close before context teardown | settle existing bounded reports, classify retained requests once, then original close/report ownership |

The new exception requires the same Request, HTTP200, exact `/mcp-resources`,
`net::ERR_ABORTED`, complete consumed bytes/EOF, verified length/digest and the
same uncanceled live mount. Other errors and unobserved cross-origin failures
remain failures. In this opt-in check, unobserved endpoint failures also fail;
no endpoint failure can be rescued by an old pattern or full-size hint. Other
checks keep their existing origin rules.

The request-specific lines are deferred until close rather than drained into an
earlier result that could not be revoked. Other page lines retain their current
step attribution. Measurement ends before the context's own disposal; it is not
an assertion that a successful app can never subsequently close.

Tests cover each ordering, duplicate URLs/requests, missing/forged proof,
metadata limits, corruption, truncation, caller/deadline/reader cancellation,
obsolete owner and cancellation after live. Browser controls run the real app in
Chromium and WebKit, with deliberate faults reported as expected negative
results, never reclassified as successful delivery. A synthetic complete-read
abort checks the classifier deterministically; naturally occurring aborts are
reported separately. Old failed receipts remain historical failures.

The separate #981 animation timeout receives failure-only diagnostics before
page close: animation state, bounded RAF samples, pending request metadata and a
screenshot. Its original predicate, 3000ms deadline and failure/exit outcome do
not change. A diagnostic capture is not a harmless classification or a source
cause attribution.

Implementation contract:

- Before invoking the original bridge factory, wrap that construction's server
  port and view callback. Capture the actual address and AbortSignal, including
  constructor-time reads. Observe the original mount until cutoff, independently
  of the inner request signal, including cancellation during client SHA256.
- Observe original promises with nonthrowing reactions and return those exact
  promises. Browser sequence numbers describe causal order; one bounded delivery
  chain orders binding calls. No application read awaits an observer receipt.
- At finalization a synchronous browser cutoff snapshots original signals and
  live owners and stops admission. Its acknowledged final sequence is required
  within the existing 2000ms settlement budget shared with size reports. Missing
  acknowledgement fails all candidate evidence. Ordinary lines drain afterward.
  Opening failure, no reporter and repeated close use the same finalizer.
- Defer every possible endpoint Request before legacy rules, even
  when its producer report has not arrived. Known invalid or ambiguous evidence
  goes straight to errors at final reporting, bypassing patterns and size hints.
  Unobserved endpoint failures also stay errors in the opt-in check.
- Intercept exact Vite source modules before navigation, validate successful
  JavaScript responses and exactly one supported named export, and decorate that
  export through a single harness module instance. Require all three factory
  activations and actual accepted live reads on this page generation. Navigation
  invalidates the generation; no old proof may cross it.
- Observer limits are 128 mounts, 256 reads, 512 retained Requests, 4 MiB per
  chunk/read, 64 MiB total consumed bytes, 128 queued messages, 8 MiB queued
  chunk bytes and 4096 total messages. Check before copying/marshalling. Overflow
  permanently fails observation and stops chunks without changing the app read.
  No identity eviction can turn a duplicate into a new eligible read.
- Correlation uses the actual fetch-init ticket and actual GET Request header,
  privately in memory; OPTIONS cannot bind. Duplicate redemption poisons all
  candidates through cutoff. Binding messages require the expected main frame,
  page generation, monotonic sequence and exact schema. Output contains no
  ticket, header, body or arbitrary peer-provided string.

Module ownership: `scripts/lib/open-failure-evidence.mjs` owns failure-only
opening capture. Resource observer, collection and served-module installation
are harness modules; `browser.mjs` alone owns request classification and
`page-lines.mjs` alone owns reporting.
