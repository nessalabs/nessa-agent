# Required gate applicability and owner validation handoff

Foundation source: `b8568beee166680a45a192222da468234933411d`.
Published foundation head: `6c38915d5201de4cac54500ec4b5ad5e5f34229e`.
Base: `0b8019cf45f99b285f10c41199875d474492c6d1`.
Dependent admission source: `d4b6e6cba2dd345c129d00f4115d272944f0f079`;
combined published admission/TLS head: `777da4336084b69e84deefc6f52081160093df37`.
Durable storage source is committed at `c608eaa2de4e24143d349f9eb60fe63b74f34193`;
its independent review and full integration composition remain open.
These are separate checkpoints, not a full issue acceptance claim.

## Applicability

CODING_STANDARDS' browser verification section requires canonical scripted
Chromium **and** WebKit evidence for gateway behavior changes. An unmounted
foundation does not by itself waive this gate. The foundation's Rust mechanism
and TLS checks passed locally; the canonical browser gate is **not passed**.
The dependent admission/composition change also requires the scripted gate.

`pnpm test:e2e:scripted -- --channel bundled --evidence
/tmp/nessa-scripted-foundation-6c38915d` ran in the clean foundation worktree at
6c38915d, using the repository's locked dependencies, skipped browser downloads,
Rust 1.99.1 and the shared Cargo target directory. The server compiled. Exit 2,
verdict `could-not-run`: pinned Chromium headless-shell 1243 and WebKit 2359 were
absent. Subsequent gateway scenarios could not find/start the binary in that
worktree (the build used the shared target directory). See
[exact summary](scripted-summary.md) and [per-check results](scripted-results.json).
No real account or persistent credentials were used. Browser downloads had
previously returned DomainForbidden; no alternate download was attempted.
Installed system Chromium used to render documentation does not count as this
canonical Chromium/WebKit verification.

Gate 19 applies to grant filtering and revocation through conversations. Existing
`conversation-stress.mjs` steps exercise gateway restart (SIGKILL/SIGTERM), bursts,
source following, bounded watch/read and switching. `conversation-chaos.mjs`
exercises restart, disconnect, uncertain command acknowledgement, source follower
ordering and access changes. Those are reused infrastructure paths; they do not
exercise an outbound recipient or registered integration credential. Chaos
explicitly leaves a second person's share checks elsewhere. A feature-specific
capability/revoke delivery E2E and a stress step at default and raised counts
remain required for the usable integration slice. The current standalone signing,
HTTPS and admission tests do not substitute for those gates.

## Owner execution inputs (not dispatched here)

The coordinator reports workflow lookup/dispatch Forbidden. No further workflow
lookup, dispatch, alternative trigger or retry push was attempted after that
instruction. The following identifies the held validation target; it is not authorization
to retry or trigger an equivalent run:

- repository `nessalabs/nessa-agent`, draft PR #1091;
- workflow `.github/workflows/local-auth.yml`;
- ref `codex/896-signed-webhook`, exact head
  `6c38915d5201de4cac54500ec4b5ad5e5f34229e`;
- sole workflow-dispatch input `rust-cache: restore` (as defined at this head);
- exact-head run last known before the restriction: [39925346689](https://github.com/nessalabs/nessa-agent/actions/runs/39925346689),
  completed failed, as confirmed by the parent;
- run `pnpm test:e2e:scripted` on that exact head with both repository-pinned
  browser engines available, retaining the emitted evidence directory and
  `pr-summary.md` in the review handoff.

No source/ref input exists for a completed integration slice yet. Record its
reviewed head and run the same applicable gates after composition and E2E land.
No merge is authorized to this worker; the parent owns guarded integration.

## Parent-supplied hosted result

After the last worker lookup, the parent reported exact-head run 39925346689,
stress job 120322813252, failed at 09:03 UTC: **Install the pinned browsers and
their runtime dependencies timed out after 15 minutes**. Gateway stress/browser
assertions did not run; the later chaos failure followed the missing build.
This is the #1090 setup failure, separate from #1092's upgrade runtime symptom.
No worker workflow lookup/dispatch followed this report. No timeout or browser
assertion was weakened. Parent subsequently confirmed the full run is terminal: all other jobs passed,
while browser dependency setup and the aggregate gate failed. A single-job retry
of `120322813252` was denied, including one evidence-backed retry after verifying
there was no newer exact-head run. Stop that action: no rerun, CLI route, dispatch
or no-op push may trigger equivalent CI. Parent owns requesting further user
approval for that narrow retry. Hosted browser/stress acceptance remains unmet.
