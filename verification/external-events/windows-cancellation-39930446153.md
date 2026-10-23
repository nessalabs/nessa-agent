# Windows cancellation-test checkpoint

Run: https://github.com/nessalabs/nessa-agent/actions/runs/39930446153
Windows job: https://github.com/nessalabs/nessa-agent/actions/runs/39930446153/job/120337505616
Browser/stress job: https://github.com/nessalabs/nessa-agent/actions/runs/39930446153/job/120337505456

The checkout log identifies merge `b8613a533571696e9a9efaf34625e23c5984c706`,
head `a1994569286c96d6be09b9bcc9d83e2f5993ffb3` into base
`0b8019cf45f99b285f10c41199875d474492c6d1`.
All checks except the Windows server-library test job passed.

## Historical browser and stress result

The canonical command printed:

```text
Verdict: pass

| Check | Chromium | WebKit |
| --- | --- | --- |
| resize-observers | pass | pass |
| mcp-apps-gateway | pass | pass |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console chromium: requestfailed: http://127.0.0.1:38879/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- console chromium: requestfailed: http://127.0.0.1:38879/mcp-resources net::ERR_ABORTED (aborted after a 200 response, full body, #473) (harmless)
{"verdict":"pass","evidence":"/home/runner/work/nessa-agent/nessa-agent-stress-evidence/scripted","summary":"/home/runner/work/nessa-agent/nessa-agent-stress-evidence/scripted/pr-summary.md"}
##[group]Run node verification/desktop/scripts/usage-limit.mjs --channel bundled --out "$STRESS_EVIDENCE/usage-limit.json" --shots "$STRESS_EVIDENCE/usage-limit"
[36;1mnode verification/desktop/scripts/usage-limit.mjs --channel bundled --out "$STRESS_EVIDENCE/usage-limit.json" --shots "$STRESS_EVIDENCE/usage-limit"[0m
shell: /usr/bin/bash -e {0}
env:
  STRESS_EVIDENCE: /home/runner/work/nessa-agent/nessa-agent-stress-evidence
  CARGO_HOME: /home/runner/.cargo
```

The same job subsequently printed `conversation-stress: 42/42 held` and
`conversation-chaos: 2/2 runs held`. These are actual results for that recorded
candidate, not qualification of a later head. The earlier run39925277268 upgrade
failure remains unattributed; this later pass does not establish its cause.
Artifact12489422730 materialization remains denied403; no retry/reroute occurred.

## Sole Windows failure

```text
external_events::tests::cancelled_http_responses_keep_command_capacity_until_detached_admission_settles
  ingress.rs:832:6: unwrap Err Elapsed(())
3030 passed; 1 failed; 8 ignored; 1408.01 seconds
```

The timed wait covered a ninth request's durable admission while eight released
SQLite commands also advanced. This mixed capacity recovery with filesystem
latency. The log does not establish a production permit leak or prove why this
particular Windows schedule exceeded the bound.

## Scoped test correction

Source `7763614c1998cc428a9b6043cbe1ab10cebe6ec4` separates the contracts:

- Eight blocked store opens retain admission slots after all response futures
  are cancelled and joined. The ninth remains busy without entering the store.
- Releasing those opens into a deliberate pre-storage failure allows a later
  request to enter the store and report dispatch uncertainty rather than busy;
  the observed counts require all eight original failures and the recovery call,
  with zero provider executions.
- A separate single-command test releases into real SQLite, observes successful
  terminal persistence through the fixture's existing storage port wrapper,
  awaits execution completion and replays the same receipt with one execution.

No timeouts increased and no production admission code changed. The prior
caller-loss mutation still targets the unchanged HTTP owner. Linux execution and
repeat tests support this correction; Windows remains pending exact-head hosted
qualification. No workflow dispatch, blind CI rerun, trigger change, artifact
retry or unrelated frontend/settings change occurred.

