# Native load acceptance fixture

This disposable macOS app checks the real load deadline, native nil URL,
diagnostic URL and preserved windows. It uses the production navigation and
load modules. `server.rs` owns its HTTP control; `probe.rs` observes native
callbacks and requires that the control request reached this process.

The HTTP control keeps its existing 8192-byte request bound, one-second read
budget, one-second write timeout and fixed allowed port 1420. TCP reads do not
delimit HTTP requests. A successful control page therefore follows a complete
header, rather than the first chunk returned by `read`.

| Input/ordering | Result | Regression |
| --- | --- | --- |
| Complete control GET | Record the request witness, then send 200 | `complete_control_records_the_request_before_success` |
| Control header split across TCP reads | Wait within the same read budget; record only the complete request | `fragmented_control_does_not_receive_an_early_response` |
| Different path or method | 404, no control witness | `other_requests_do_not_publish_a_control_witness` |
| EOF before the header ends | No success response or witness | `incomplete_and_oversized_headers_cannot_publish_success` |
| Header past 8192 bytes | Refuse within the bound, no success or witness | `incomplete_and_oversized_headers_cannot_publish_success` |
| Peer stops partway through a header | Read timeout, no success or witness | `stalled_header_keeps_the_original_read_budget` |

The portable server regressions compile this standard-library-only module
through `scripts/desktop/page-load-server.test.mjs`. They do not substitute for
the real macOS example in CI.

CI run 39513993837 failed after the native control finished without its request
witness. Main run 39511867651 passed on the same macos-26-arm64 image
20261013.0424.1, Rust 1.99.1 and cache key, with identical fixture source.
A fragmented-request probe reproduces a concrete fixture defect: the former
single-read server returned 200 after only `G`, without setting its witness.
The CI request bytes were not captured, so that original failure's precise
cause remains unestablished. Keep that failure alongside fresh qualification.
