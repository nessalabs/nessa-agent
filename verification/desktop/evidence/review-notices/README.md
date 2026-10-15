# Review-notice verification (#918, #920)

Current notice source: `ce0dc3bffee2697bfb9775bd39dbd8499fb257cc`.
Base: main `4219790db1827ee841eacfaf73c00605eec6bc07`.

The gateway was rebuilt from assembly
`0d7bd5e6218af559dde93295f85b05009a91e6e0`, tree
`5a1e5be8d9e83a6587449cd84c9e2a8da74ce4f7`. It combines that main, the current
notice source, and audit source `c11524cb6693fe50784b97ed3e8a8f898e8f7966`
from #928. Both integrations were conflict-free. This verifies the final notice
implementation; #928's later attachment-observation correction remains under separate
verification and is not claimed by this run. The evidence-only follow-up changes
no executable source.

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

These four inspected JPEGs replace the earlier integrated captures. They are
resized to 1440 pixels wide from this run's screenshots. Permission images show
the permitted recipient's notice and review card; gateway images show the live
conversation flow. The Rust wire regressions establish membership revocation and
connection-local invalidation rather than screenshots alone.

![Current permission review in Chromium](integrated-permission-chromium.jpg)

![Current permission review in WebKit](integrated-permission-webkit.jpg)

![Current gateway in Chromium](integrated-gateway-chromium.jpg)

![Current gateway in WebKit](integrated-gateway-webkit.jpg)

## Current source checks

The exact notice source passed 70 review tests, 26 Cedar policy tests and
295 socket tests, with one existing latency measurement ignored. These suites
overlap. The exact seven-package CI selection passed all-target Clippy with
`-D warnings`; formatting and architecture checks passed. A fresh independent
full source review found no actionable findings at any priority.

The actual long-lived socket regression checks owned/foreign notices, demotion,
promotion, disabled membership, unchanged board generation and increasing wire
sequence. Two live sockets and policy-read counts prove one connection's refresh
does not wake another publisher, while shared visibility and board changes still
wake both. Separate cases retain authority changes preloaded or received during
metadata filtering and blocked publication, and end on sender closure. Each
connection may still observe the global registry revision once; the correction
removes repeated global fanout rather than claiming principal-specific revisions.

The PR discussion tracks current-source reversal execution and its disposition.
The historical reversals below do not establish the local-watch guarantee.
Current-head CI and external review remain separate merge gates.

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
