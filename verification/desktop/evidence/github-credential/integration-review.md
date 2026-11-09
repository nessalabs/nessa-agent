# Integrated broker review

Source: `499b5afd7e34f50e425302b6c951b0d817cd8cdd` against main
`d8a6c0f7b104126124b7b645f77fbf2f1ce0a8ba`. This is source-review evidence;
executed checks and environment exclusions are in [the evidence index](README.md).
Published source commit `25f1e3526bd9dda8668c299bf7e40e5bdf22e28f` has the identical
Git tree `442038a5bf159f15222f36c5e52414363511a157`; only commit metadata differs.
It merges main into the existing PR branch, with no history rewrite.

## Independent integration review

Reviewed composition, provider opens, managed MCP and shell environments, Cedar
source admission, encrypted persistence/migration, and product Disconnect's
live-event cleanup. No functional integration defect found. The relay still uses
`ConnectedGithubCredentials` and the generation-fenced `GithubConnections::token`;
that accessor remains necessary for gateway services. Provider configuration uses
`OpenCredentials::none`, and no managed-shell launch credential injection remains.
Background tools remain composed and attached.

Minor findings, all fixed:

- The GitHub module map still named OS credential storage. It now names the
  encrypted app-session vault.
- GitHub composition still described agent-open credential reads. It now names
  the gateway broker and live-event relay.
- Tool parsing duplicated positive issue/PR number validation. It now asks
  `GithubRepository::path`, retaining the zero-number rejection regression.

This reviewer excluded the actively changing cancellation code and ran no builds
or browser checks.

## Independent adversarial cancellation review

Reviewed the final source commit, including the minor dispositions above. No
remaining source findings. Traced cancellation through gate admission, token
access, adapter dispatch/drop, same-generation Disconnect, newer-generation
progress and stale-response refusal. All three fixed GET kinds share that owner.
Sender closure fails closed. The production HTTP adapter spawns no detached read
work, so its future is dropped before the held generation guard is released.

The reviewer found a missing simultaneous-readiness regression. It is addressed
by `cancellation_wins_when_the_adapter_answer_is_also_ready`, which covers all
three GET kinds. Reversing the second select's priority produced
`Ok(Object {"id": Number(1)})` instead of `Err(Cancelled)`. The source was restored
exactly before the final checks.

`reads.rs` forces the queued, active, both-ready and vault replacement orderings.
`read_http.rs` uses synthetic credentials and a real loopback HTTP socket to
confirm active request cancellation, queued non-dispatch, socket closure and
Disconnect. The original integrated-but-unfixed method failed the pre-dispatch,
queued, active and stale-response regressions (82 passed, 4 failed). Its repaired
version initially passed all 87 focused tests; the added priority regression is
qualified by the later final-tree run recorded in the index.

Outer caller abort and cross-platform behavior were reviewed by source only.
Neither review submitted public approval or merged the PR. No claim is made about
OS isolation of an unlocked same-user process, remote rollback of an already-sent
GET, or brokered GitHub writes.
