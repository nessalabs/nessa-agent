# Question targets — issue 797

The client captures one published ask with its enclosing conversation, execution,
question identity and offered fields/options. `QuestionTarget` is immutable and
process-local; its factory accepts a validated view or trusted local projection,
not arbitrary IDs. The panel renders the captured ask and carries it through a
single deliberate answer or decline. The SDK remains the owner of required,
free-text, selection-count, pending-state and provider rules. Capturing intent
neither authenticates a fabricated view nor grants authority.

## Ordering and enforcement

| Row | Input or ordering | Required result | Enforcer |
| --- | --- | --- | --- |
| QT1 | Capture asks from distinct conversations, turns and invocations | Preserve each original combination | `questionTargets` |
| QT2 | Mutate source identities, fields or options after capture | Rendering and submission retain original published ask | Frozen copied projection |
| QT3 | Copy, forge or independently replace an identity | Refuse before preparation/RPC | Private issuance membership |
| QT4 | Submit another ask's field or option | Refuse before RPC | Captured published field/option correlation |
| QT5 | Correct answer or null decline | Preserve existing wire and attribution contract | Conversation API |
| QT6 | Active tab changes while preparation waits | Submit captured ask to captured conversation | Control argument |
| QT7 | Route to another tab or reuse local tab before dispatch | Refuse before preparation/control | Conversation agreement check |
| QT8 | Pending state or authority changes after capture | Existing server refusal, no replay | SDK/gateway owners |
| QT9 | Another conversation publishes equal turn/ask names | Do not inherit the previous conversation’s answer draft | Full target identity in React key |
| QT10 | Simultaneous asks reuse an agent field key | Selecting one ask does not unset another ask’s selection | Questionnaire primitive generates each native group identity |
| QT11 | Caller mutates answer choices while transport serialization waits | Serialize the original choice snapshot that passed correlation | Deep copied/frozen choices before `questionFields`, same snapshot passed to RPC |
| QT14 | A caller supplies boxed/object scalars whose serialization can change later | Refuse mutable scalar representations before issuing targets or effects; ordinary strings/booleans and published nullable text remain accepted | Primitive representation checks in the capture owner, not provider answer policy |
| QT13 | Caller replaces the command kind/target/choices while preparation waits | Apply the original question target and answer, or original decline | Frozen question control using `captureQuestionChoices` before first await |
| QT12 | Source arrays override map/iteration, or source records carry getters/serialization hooks | Capture only published data fields into ordinary owned records/arrays at their initial length, reading each field once; later source mutation cannot replace captured data | Index copies and explicit refusal of missing elements before freeze; same answer snapshot for correlation and RPC |

Tests in `question-target.test.ts` and store `question-targets.test.ts` exercise
QT1–QT7/QT11–QT12; store tests also exercise QT13; scalar representation tests cover QT14. `conversation-questions.test.tsx` covers QT9/QT10. Existing acknowledgement
and SDK tests retain QT8 ownership. Local-tab
replacement during already-admitted preparation belongs to the tab lifecycle
owner and is separate from capture. No wire schema or provider policy changes.
