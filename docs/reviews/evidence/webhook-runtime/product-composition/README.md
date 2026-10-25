# Retained product webhook commands and grants

Source: `20d8051b5e03419d7d0d1854413d7e29be1b6207`, preserving published
locking repair `15e211372` and main `d3dbf580`. This is a product command/grant
ownership slice. Canonical source/queue/sender mounting and recipient POST remain
unfinished. No delivery or whole-feature qualification is claimed.

The actual loopback WebSocket boundary exercises owner authentication,
credential.issue, private registration, conversation.share/unshare and revoke.
Native Auth/Cedar, private SQLite and actual canonical grant writes are used;
no Agent/provider is opened. A webhook bearer is refused product-session entry.
An ordinary authenticated nonowner cannot share the hook; ordinary unpaired
targets remain unpaired. Ordinary issue/replay/revoke preserve their wire results.

The first review found missing host cleanup ownership, missing lifecycle/config
controls, three Clippy errors and two nits. [Original report](review-1.md).
Host cleanup now records an independent typed webhook outcome. It polls that
owner first to seal admission and polls other drains while physical work joins.
Unknown, failed or deadline-exceeded evidence prevents confirmed cleanup.

Two additional author regressions were caught and preserved:

- The first worker panic woke its response observer before sealing admission.
  Original result: 0 passed/1 failed, with `sealed_when_woken=Some(false)`.
  The corrected owner seals before the actual response wake; the regression passes.
- Production cleanup completed while a real canonical share write remained held
  inside its blocking worker. Original result: 0 passed/1 failed. Existing
  registered-webhook share/unshare operations now run in the same retained job
  owner. The corrected regression passes both physical mutation cases.

Other tests cover actual issued-but-uninstalled recovery after process-store
close/reopen, original intent/key/destination reuse, no recovered/new bearer,
caller loss, shutdown observer cancellation, capacity before spawn, independent
control admission, issuance serialization and original request identity.
Configuration tests exercise real preparation, refused-input/no-file-effects,
64 valid seeds without issuance, seed removal/reopening, redacted diagnostics,
and non-file/symlink refusal.

Final author checks: 235 webhook tests passed (the three unchanged full-boundary
retention cases were excluded locally and retain their earlier qualification),
6 configuration tests, 5 shutdown-report tests, 27 host cleanup tests,
seven-package all-target Clippy `-D warnings`, formatting, 108 architecture tests
and architecture check passed. Ten of the webhook cases use the actual product
boundary. Local checks are Linux evidence, not new Windows/macOS qualification.

[Six one-rule controls](controls.jsonl) independently caught excess ordinary
capacity, control-lane coupling, omitted issuance serialization, substituted
recovery input, omitted host joining and weakened seed preflight. Every source
byte was restored. After adding the necessary retained-grant helper, removing
only that helper/import from runtime.rs reproduces the recorded baseline SHA256
`133089b834c9513cfe6050bd5abf67a9a3278fac5f79eb2f42c1b19c6c88046b`;
all previously tested runtime rules are byte-identical. Host/config control
sources remain exactly equal to their recorded hashes. The new grant-retention
rule has its separate original-failure/corrected actual-physical-write regression.

Original fixture compile failures and all raw original/corrected/mutation logs
remain preserved in the execution workspace. Review findings must be closed by
a fresh full round before treating this slice as independently reviewed. Browser,
stress and recipient source qualification remain separate outstanding gates.

Round-two closure: [original report](review-2.md). The new public target
classification now invokes existing caller attribution validation first, and
unshare asks the existing Cedar Unshare action, preserving deleted-conversation
cleanup for both webhook and ordinary departed-receiver grants. Both actual
product regressions failed on the original candidate and pass after correction;
the latter checks canonical row removal. The attribution control covers blank,
control-character and oversized request IDs on missing and deleted conversations.
The original attribution test first had an incorrect expected wire literal; its
corrected red control uses the existing error mapping and still fails before
the fix. All 12 product tests, server all-target Clippy, formatting, 108
architecture tests and architecture check pass. Other previously qualified source
is unchanged; no unchanged retention or mutation suite was repeated. A fresh
full third review remains pending.
