# Retained outbox cleanup cost

Source `611b5a87ae3e0256f50e72b16bc4a1e4cc5e4d49`, base `0e1f44def810c8b91439000f52986869cb0cdfc2`. This is a scoped SQL cleanup correction for #896, not a complete runtime or a verified Windows repair.

Old-head [Windows job 120586159654](https://github.com/nessalabs/nessa-agent/actions/runs/40003690156/job/120586159654) ended at the 45-minute test-command timeout. Both retention tests were still running after other outbox tests finished. [Last progress](before/windows-job-last-progress.txt) does not demonstrate a deadlock. No CI rerun, dispatch, timeout change or browser-download retry was attempted.

The old cleanup repeatedly ranked retained rows and scanned published delivery audit history. The current owner computes one materialized indexed cutoff per registration, keeps the newest 1024 terminal rows, and still refuses deletion while any audit is owed. An owned trigger removes only the deleted terminal delivery's published audit; queued/sending/retry history and owed rows are retained. Existing source-reset reference cleanup remains transactional. Registration audit cleanup uses a separate partial index. No credential, grant, signature, retry, acknowledgement, flush or publication ordering changes.

[Source proof](source-checkpoint.json) verifies the original full 1024-row boundary test bodies and production bounds are byte-identical. New [cleanup tests](../../../../../crates/nessa-server/tests/webhooks/infrastructure/outbox/cleanup.rs) measure SQLite VM work and exercise published/owed/nonterminal audit ownership. Their cost datasets are explicitly SQL metadata fixtures, not authenticated replay or audit-publication fixtures. Actual persistence, failure, reset and full-boundary behavior remains covered by the canonical suite.

| Old-head / repaired Linux profile | Before | After |
| --- | ---: | ---: |
| Terminal fixture cumulative approximate VM instructions |644563700|243223000|
| Stop-retention setup approximate VM instructions |698864800|308314700|
| Terminal fixture fsync calls |30772|30772|
| Stop-retention fixture fsync calls |36920|36920|

Profiles use byte-identical instrumented full-boundary test bodies, actual source and independent Auth compilation, with strace and a noninterrupting SQLite progress counter. [After source/fixture proof](after/source-match.json) records hashes. Approximate VM counts are comparative work evidence. Traced Linux elapsed times196.43→28.70s and180.05→27.31s are subject to tracing/load effects and do not qualify Windows. Flush counts remain unchanged, so platform filesystem cost remains a risk until an allowed exact-head CI run.

Source checks: 120 canonical webhooks passed;212 auth unit + 55 integration passed; required seven-package all-target locked Clippy -D warnings,architecture,fmt anddiff checks passed. Isolated 90 tests passed. [Nine mutation controls](controls/results.json) each failed the intended regression and passed after byte-identical restoration: ranking, history scan, owed guard, delivery audit retirement, published-only and terminal-only ownership, materialization, terminal index and registration index.

One private restore compile overlapped canonical Cargo's shared archive rebuild and failed linking missing archives. The [original failure](controls/materialized-cutoff-restored-compile.txt) is retained; it was not counted as a successful control. After canonical builds finished, actual Auth was recompiled against a frozen dependency snapshot; remaining controls and the complete 90-test restored harness passed. Snapshot bytes are not committed. No shared target writes are allowed during independent review.

State-page validation passed:135 pages,zero issues. Both owning diagrams rendered with zero page errors and were visually inspected in system Chromium 151.0.7922.173, at the exact source checkpoint. Fresh independent review and publication are pending at this evidence checkpoint. Pinned Chromium/WebKit, gateway scripted/stress/chaos and exact-head CI remain applicable gates. System Chromium documentation renders do not qualify those gates. The foundation retry/report/browser denials remain binding. Runtime mounting, W24 coordination and recipient capability/revoke E2E are separate unfinished work. No live registration, credential, recipient, production data or public listener was used.
