# Independent consumer review: issue #712

Reviewer: primary agent /root, independent of the consumer implementation author.
Reviewed head: 9c1c672f2f20e39f63ff9745a32d6467259d328d.
Reviewed base: bd8ef0763b92a6f5ca6a7178d736b21a7a5ffe17.
Scope: the five-file consumer delta in send-draft.ts, panel/ui/app.tsx, read-failure.test.ts, attachments.test.ts, and ADR 231. This is a delta review, not a renewed review of the earlier API/native implementation.

Result: zero actionable findings at every priority. Review round ends here.

The existing draft-decline decision owns the unavailable bound-view rejection, after session connectivity and before attachment policy. It retains draft state and stops create/send/steer effects. The panel disables the existing bound approval control from the same published read-error fact. ComposerTray applies disabled to both the parent control and its mode choices. Unbound local drafts preserve their existing behavior. Valid refollow recovery clears the failure and restores submission and approval controls. The change adds no wire contract, history/paging behavior, source-read API, or competing authorization owner.

Inspected the complete delta and surrounding sendDraft, followTab/request fencing, applyView recovery, bound-versus-local panel branches, and ComposerTray enforcement. Typed public read failures are defined nonempty values; the undefined check matches the published failure state.

Fresh scoped verification at the reviewed head: 94/94 tests in three files (54 attachments, 10 read-failure, 30 composer-tray). Log: /tmp/nessa-712-review-scoped.log.

Two independent temporary public-store probes passed, with the full read-failure file at 12/12: captured unavailable input stays rejected with its draft and zero effects even if synchronous retry recovers; input admitted before a later follower failure is not retroactively revoked. Log: /tmp/nessa-712-review/probe.log. Actual probe diff: /tmp/nessa-712-review/probe.diff. Source was restored byte-for-byte with fresh mtime; restoration.json records SHA256 9fe0a77e5d22efb4143bad573eb03732a1b8fff653f68228879f69f4e42a5b36. These counts overlap the normal ten tests and are not additive.

Supporting author evidence inspected: /tmp/712-unavailable-draft/author-report.md and actual source-removal failures, restoration identities, and unchanged mode-publication fixture. Author mode result 3/3 means Chromium and WebKit consumer rows plus the console row in one run, not three repeated trials. This supporting browser evidence belongs to author source f803d375a1036d73016b38cd6822e8c9e2947237, integrated as the reviewed head's identical consumer delta.

This reviewer did not run browser, production performance, full run-all, Rust/native/Windows checks, or current-main compatibility checks. Those remain separately reported gates, with no approval implied by this scoped review. Earlier broader reviews are not silently relabeled as current-head reviews.

Final checkout is detached at the reviewed head, git status is clean, git diff --check passes, all temporary changes restored, and all reviewer-owned test/compiler/browser processes have ended. The coordinator and browser delegate may proceed.
