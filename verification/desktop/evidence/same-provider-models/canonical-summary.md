Source: `81311484005f40cd6a48aafee535be2ad9d07876`, assembled immutable main5813.
Canonical gateway SHA256: `25b647ca5e09eaada808532001f5bc3446a04e94f8911c80dc914cdf59bd2deb`.
Later stress/chaos gateway SHA256: `47d7ec4c56cebaf485a61ff9751124d048097150a4b94aa14aca5f752f1c629d` (own native_enrollment integration build before journeys).
Fixture-only final correction preserves production and canonical-program source; artifacts are not claimed byte-equal.

Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console chromium: requestfailed: http://127.0.0.1:60321/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
