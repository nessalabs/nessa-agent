# Daybreak remediation and regression verification

Initial review: feature 079798f614e59f28848a27af8121befef5ee0349,
prospective merge cf9b388247f22867c7ff48034e257d239c6dedef.
Report SHA256: 40eda4258e2563ffb9f581b9c1a2348acc9cc5239c8f883299e264181c9aba8f.

The initial review blocked merge on complete/current Boat admission, unbounded
relay refusals/watch messages, and ordinary long-lived GitHub token copies.
The fixes are committed at code `05bb0383823d28e37f563ba28cea72fa9c6de9a3`.
The [fresh closing review](daybreak-closing-review.md) at clean `fa0449da9`
reports no actionable findings; the [initial review](daybreak-initial-review.md)
retains the original counterexamples. Hosted CI must pass on the published head
before merge.

- Boat checks one explicit agent-only posture, default profile/null and false
  safe-mode proxy; current immutable environment ID/version is matched before
  create and before/after resume, and concrete VM metadata is verified before
  workspace SDK/SSH/MCP authority. Personal scope is explicit.
- Read-only personal-scope v1 API metadata confirmed ResourceAccess is an object
  and supplied the effective flags. The sanitized environment fixture preserves
  that shape. It contains no live IDs, contents, files or repository data.
- Complete bounded create JSON retains its valid target before typed decoding;
  malformed fields cannot erase the recovery target. Incomplete/oversized bodies
  remain uncertain intents without an invented target or automatic replacement.
- Only an acknowledged pending allocation may roll back. Existing workspaces,
  ready/idempotent replays and unrecognized acknowledgements are retained on
  refusal or receipt-save failure. Refusal histories are terminal; old metadata
  without proof is read only as Unverified. Audit retains target, expected and
  bounded observed identity, cause, disposition and cleanup outcome.
- GitHub credentials remain the existing zeroizing/redacted AccessToken and are
  acquired per page request. The first page and zeroizing watch ticket leave
  scope before idle watching. HTTP200/409/410 share one streamed body bound;
  explicit WebSocket frame/message bounds admit valid fragmented changed and
  full ping/pong frames. The library additionally bounds handshake size/packets.

Current standard checks at code commit 05bb0383823d28e37f563ba28cea72fa9c6de9a3:
39 workspace tests, 76 GitHub/relay/product tests (including 29 relay tests), one real admission-audit test,
one real receipt-audit test, eight-package all-target Clippy with -D warnings.
Architecture passed. Docs validated 136 pages, zero errors, 33 existing metadata
warnings. Concrete revert probes rejected relaxed relay frame/message limits and
credentials held across the follower interval; the restored 29-test relay suite
and full 76-test GitHub suite pass. Isolated domain probes reject each relaxed
posture, identity and history guard. Daybreak independently reran eight admission, sixteen allocation, three relevant
watch and one oversized-refusal test. Nine total tests matched its watch filter.
Its report records the exact source, fresh checks, relied-on evidence and limits.

No Worker, D1, secret, Cloudflare plan or live VM was created/changed for these
fixes. Provider metadata queries were read-only. All 43 existing worker lease
inputs still match b6e4f48aa694ce08; no worker pin or guard was relaxed.

No E2EE, provider secure-erasure or whole-process memory-erasure claim. The API
has no immutable-version create selector; provider/admin mutation during a
name-based create remains a trust boundary. Native audit-cap #1185 and pane-root
#1184/separate draft #1174 remain outside these fixes. Current main 2f0e4db7e
adds only rail CSS/TSX/stress changes, with no security/lease-input overlap.

## Documentation checkpoint and exclusions

The two affected state pages, channel workspaces and GitHub webhooks, were
reviewed against their previous checkpoints and current source/tests; their
`sourceRevision` is the exact code commit `05bb0383823d28e37f563ba28cea72fa9c6de9a3`.
Architecture and owning module maps were updated with the new admission/refusal
and transport responsibilities. This evidence-only update changes no state
behavior, so it does not advance those checkpoints or unrelated pages. The
Nessa Docs catalog remains 136 pages, zero errors and 33 known metadata warnings;
no new visual renderer inspection is claimed.

The regression fixture is a provider schema snapshot, not a live-provider
attestation. Legitimate unknown fields deliberately refuse until reviewed.
Historical browser/provider results in the phone completion record retain their
original source and environment, rather than being relabeled as these security
fixes. No UI code changed in these repairs. The required hosted platform and
conversation checks still gate final landing.
