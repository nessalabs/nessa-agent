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

Tests in `question-target.test.ts` and store `question-targets.test.ts` exercise
QT1–QT7; `conversation-questions.test.tsx` covers QT9. Existing acknowledgement
and SDK tests retain QT8 ownership. Local-tab
replacement during already-admitted preparation belongs to the tab lifecycle
owner and is separate from capture. No wire schema or provider policy changes.
