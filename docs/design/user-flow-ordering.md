# Frontend pending action ownership (#1087)

This correction follows the source baseline
`0b8019cf45f99b285f10c41199875d474492c6d1`.
The independent audit reproduced two frontend ordering defects. It did not
establish duplicate physical backend installation. Existing request admission,
server authorization and provider execution remain owned by their current layers.

## Installation offer publication

`useAgentDownloads` owns the presentation of gateway installation offers. It
coalesces local install clicks while awaiting a command. An uncertain outcome
clears offers because physical work may remain. That boundary must also retire
pending offer reads: a response begun before uncertainty cannot serve as the
fresh evidence required for another attempt.

The correction uses the existing offers revision, rather than adding a second
retry flag or recreating gateway installation policy. A later explicit check or
connection-triggered read remains eligible. Source replacement and unmount keep
their existing lifetime checks; they do not cancel gateway-owned installation.

| Row | Event order | Required result | Evidence |
| --- | --- | --- | --- |
| DI1 | Offers ready; two Download clicks while installation is pending | One frontend install dispatch | Existing `agent-downloads.test.tsx` click-coalescing case |
| DI2 | Offers read begins; Busy or Not confirmed arrives; old offers resolve | Retire earlier reads; Download stays absent | Two added outcome cases; browser `uncertain-install-busy` and `uncertain-install-not-confirmed` |
| DI3 | Fresh check begins after uncertain outcome | New offers may restore Download | Same outcome cases assert recovery; browser requires one restored button |
| DI4 | View/source retired; install settles later | No publication into replacement or readiness callback | Existing view/source retirement component tests |

The counterexample before correction was: start install, begin offers read,
receive Not confirmed, hide Download, apply old offers, show Download again.
That permits another frontend dispatch. It does not prove the backend would
execute another installation.

## Composer draft observation

`useComposer` owns expanded-editor presentation. The conversation store owns
message admission. The hook watches the submitted draft leave so the editor can
close before a full gateway round trip; it does not infer acceptance from the
submit handler running.

Replace its shared pending boolean with one observation carrying the captured
conversation ID. The observation object's identity correlates the promise
callback; the conversation ID correlates draft changes. Both checks are needed:
a different tab must not collapse, and an old refusal must not remove a newer
observation, including one created in the same tab.

| Row | Event order | Required result | Evidence |
| --- | --- | --- | --- |
| CO1 | Submit A; switch to B; expand B; clear B without a send | B stays expanded | Added `use-composer-ordering.test.tsx`; browser `cross-tab-clear` |
| CO2 | Submit A; submit B; A refuses; B draft leaves | B closes; A cannot clear B's observation | Added ordering test; browser `old-refusal-new-send` |
| CO3 | Submit refuses; draft cleared without a later send | Editor stays expanded; no sent callback | Added ordering test |
| CO4 | Submission proceeds in its original tab | Draft leaving closes editor; callback still names original conversation | Existing composer cases plus CO1's late accepted callback |

No provider-completion, physical-cancellation or durable-acknowledgement
semantics are added. Pending-attachment refusal remains owned by the panel
read seam. Prepared operation identity remains owned by the conversation store.

## Validation boundary

`verification/desktop/scripts/user-flow-ordering.mjs` mounts production
AgentDownloads and the production composer hook over deterministic ports. It
checks real browser event ordering without requiring gateway credentials.
A form drives the hook; outputs expose its actual expansion state. This is
frontend dispatch/presentation evidence, not gateway/provider E2E.

The scripted gateway gate and conversation stress still need a built
`nessa-server`. Existing stress send/queue/stop steps cover the unchanged
conversation admission path; the new ordering fixture covers these frontend
interleavings directly. Defaults and raised-count stress were not run here.
See [evidence](../../verification/desktop/evidence/user-flow-ordering/README.md)
for environment limits and reproducible commands.
