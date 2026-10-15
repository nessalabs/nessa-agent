# Review-notice verification (#918, #920)

Current behavioral source: `83d0730dc18b67b19e55ed55e701955ed7664b3a`, including
the graceful review-audit shutdown correction. Test-only follow-up
`4ecdf3e1af3a386fe0f15b161fe0b46ee7ac9484` removes a needless borrow; its
production files are byte-identical. This capture refresh changes no runtime behavior.

The gateway was rebuilt from assembly
`f4bc82cc4aaccb524dcb37ba1eec82c442ed790e`, tree
`23bfb1e54e6dcec64ad961a7c1150465275e52f2`. It combines that notice source,
main `cc7bacc5ed343b17f8a778446776e45a34037c43` (including merged #928),
Windows test-control repair `27f74b4b1f51c729fe34ad97227e68e7c0a55d6b` from #936,
and cloud bootstrap `ba628d0030a32f9a5231c1d92f9032d52efa3de3` from #933.
All integrations were conflict-free. Offline frozen pnpm preparation and the
current-main UI pin passed `ui:check` before the rebuilt run.

Environment: local macOS, Vite development mode, bundled Chromium and WebKit,
columns layout. `MCP_LIVE_NESSA` selected the newly built gateway.
`pnpm test:e2e:scripted -- --channel bundled --evidence <directory>` passed all
three checks in both engines. Counts combine engines and shared setup.

| Check | Chromium | WebKit | Assertions |
| --- | --- | --- | --- |
| MCP Apps through the gateway | pass | pass | 16/16 |
| Gateway window | pass | pass | 13/13 |
| Permission, allow, failure and cancel scenarios | pass | pass | 8/8 |

```text
mcp-apps-gateway: 16/16 held
gateway-window: 13/13 held
ok    permission [chromium columns]
ok    permission [webkit columns]
scripted-scenarios: 8/8 held
Verdict: pass
```

Only the existing harmless #485 Chromium `/browser/check` aborted response after
HTTP 204 appeared in the console. This functional run makes no performance claim.

## Current captures

These four inspected 1440-pixel-wide JPEGs come from the rebuilt assembly above.
Permission images show the permitted recipient's notice and review card; gateway
images show the live conversation flow. Rust wire regressions establish authority
changes, audit failure and connection teardown rather than screenshots alone.

![Current permission review in Chromium](integrated-permission-chromium.jpg)

![Current permission review in WebKit](integrated-permission-webkit.jpg)

![Current gateway in Chromium](integrated-gateway-chromium.jpg)

![Current gateway in WebKit](integrated-gateway-webkit.jpg)

## Current behavioral checks and limits

Frozen audit source `4ecdf3e1af3a386fe0f15b161fe0b46ee7ac9484` passed 109 affected
review tests, 41 composition tests, four report tests, and the exact seven-package
all-target Clippy selection with `-D warnings` under Rust 1.99.0 on local macOS.
Its eight prior owner and fourteen shutdown reversals each produced an intended
test assertion failure. Restored 109/41/4 suites passed; all 35 changed-file hashes
matched the frozen source and restored export. The earlier 28 reversals remain
qualified to `f24f4d147`, rather than this later shutdown source. Final integration
and independent review are recorded separately on the PR; screenshots do not
establish these Rust guarantees. Suites overlap.

The auditor now fences new admission during graceful cleanup, retains admitted
pre-policy operations and transferred decisions, and reports its typed drain
outcome beside the existing drains. Its first failed acknowledgement remains
failed; a deadline or lost task cannot fabricate success. Runtime/process loss
may still interrupt unacknowledged work, and an unconfirmed shutdown does not
claim physical release.

The structural regression holds an allow acknowledgement while current authority
or shared state changes, then requires a new coherent candidate before admission.
Independently owned audit work outlives publisher loss; its capacity and observation
deadlines do not claim to bound physical writes or an already-admitted UI response.
A real full control queue formerly lost the fatal close: the regression failed
before the owner supervised the publisher and passes with owner/input termination
while the peer input sender remains live. Unexpected publisher panic is covered too.

The final candidate check validates current recipient authority and board/shared
generations. It does not establish a globally atomic metadata/authority snapshot;
that inherited boundary is tracked in [#934](https://github.com/nessalabs/nessa-agent/issues/934). Durable audit uses the existing storage
platform contract: file sync and create-only publication, with directory sync where
supported; Windows does not promise directory-entry durability on power loss.
Current-head CI and external review remain separate merge gates.

## Earlier audit/admission and publisher-owner evidence (historical)

The previous integrated captures were refreshed at `f24f4d147` from behavioral
source `3f98e63c1d5643afad7beb05a9e411a43ae21a45`, assembly
`b00a3cee0dd351949e93e9032951f9aa80059d5d`, tree
`980894a0d6412cc422f34f707dce22ca0cea5702`, with main `76d2f7317` and #928
source `4864b1aa7`. Chromium/WebKit passed 16/16 MCP, 13/13 gateway-window and
8/8 scenario checks. That source passed 96 review, 26 Cedar and 317 socket tests
(one existing measurement ignored), with package Clippy, format and architecture
checks. Later f24 evidence qualified all 28 applied/restored reversals and
baseline/restored 96 passes. Those checks precede the graceful-shutdown fix.

## Earlier integrated evidence (historical)

The previous four integrated captures were refreshed at `67a9321f0` from notice
source `ce0dc3bffee2697bfb9775bd39dbd8499fb257cc`, assembly
`0d7bd5e6218af559dde93295f85b05009a91e6e0`, tree
`5a1e5be8d9e83a6587449cd84c9e2a8da74ce4f7`, with old main `4219790` and audit
`c11524cb6693fe50784b97ed3e8a8f898e8f7966`. That source passed 70 review tests,
26 policy tests and 295 socket tests (one existing ignore), plus both browser
engines with counts 16/16, 13/13 and 8/8. It predates the audit/admission/board and
publisher-supervision corrections and does not verify them.

## Earlier captures and authority-refresh evidence (historical)

The original `permission-review-*` and `gateway-live-*` JPEGs were captured from
source `8f2fd076ea1608bd925375584e2235a73cb9c79c`. The first integrated captures
were committed at `8a3ac72fd15ae9fe4d67683c89f620bdfce88fe4`, from assembly
`2f901190325d2b420e0e21f1cb5e6fbaa9db2e48` containing notice source
`42f2c982dd3767fe11fb202ecdc6453b87e15000`, main `4219790` and audit `c11524c`.
That run also passed both engines with counts 16/16, 13/13 and 8/8.

Source `42f2c982dd3767fe11fb202ecdc6453b87e15000` passed 67 review tests,
26 policy tests and 292 socket tests (one existing latency measurement ignored),
plus formatting, architecture and package Clippy. Its four executed reversals
removed authority invalidation, forced stable-revision invalidation, substituted
role-only observation, and discarded the authentication revision. Each failed its
assertion, followed by exact source restoration. Those earlier results do not
verify the later local-watch correction.

## Earlier metadata and policy reversals (historical)

Source `8f2fd076ea1608bd925375584e2235a73cb9c79c` passed `cargo test -p nessa-server --lib review`: 66 tests.
The complete Cedar access-policy suite passed 26 tests. The complete socket
suite passed 291 tests, with its existing multi-minute latency measurement
ignored. These suites overlap.

Each reversal below was applied separately to an isolated export of the exact
semantic source, with the applied hunk and SHA256 printed before execution.
Each produced a test assertion failure, rather than a compilation failure.
After each run the source was restored and given a fresh modification time.
The original checkout and the running browser gateway were not mutated.

| Rule reversed | Applied change | Observed assertion |
| --- | --- | --- |
| Metadata identity | Reader calls the raw repository instead of the checked loader | `refused.reviews.is_empty()` fails |
| Membership freshness | Read membership before awaiting metadata | `Member, Active`: actual `true`, expected `false` |
| Policy refusal | Add a Rust admin shortcut before the evaluator | Refused-policy snapshot unexpectedly contains a review |
| Owner identity | Invert Cedar's owner equality | Non-owner member is allowed |
| Admin membership | Change Cedar's admin role to member | Non-owner admin is refused |
| Organization boundary | Remove organization equality from the review permits | Foreign-organization admin is allowed |
| Live conversation | Remove deletion refusal from the review permits | Deleted conversation is allowed |

Short failure lines from the actual runs:

```text
metadata-identity: assertion failed: refused.reviews.is_empty()
membership-ordering: assertion `left == right` failed: Member, Active
policy-refusal: assertion failed: notice_snapshot(board.clone(), state, session).await.reviews.is_empty()
owner-identity: assertion `left == right` failed: Member, same org=true, owns=false, deleted=false
admin-membership: assertion `left == right` failed: Admin, same org=true, owns=false, deleted=false
same-organization: assertion `left == right` failed: Admin, same org=false, owns=false, deleted=false
live-conversation: assertion `left == right` failed: Admin, same org=true, owns=false, deleted=true
restored source: test result: ok. 65 passed; 0 failed
```

The repeated exported baseline and restored runs excluded only
`app_reviews_never_cost_the_agents_own_however_the_view_is_filled`, which had
already passed in the original 66-test run. No review-notice regression was
excluded.
