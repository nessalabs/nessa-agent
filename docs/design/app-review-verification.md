# App review verification

The app-review browser fixture owns its simulated pending app request. The
production gateway adapter supplies the SDK's `answer(target, optionId)` call.
`FakeGateway.calls` narrows answer arguments to
`Parameters<ConversationApi["answer"]>`; `onceAnswer` delivers that captured call
and a normal SDK result directly. Other fake controls keep their existing shape.
The fixture's snapshot projects named diagnostic fields, not an obsolete tuple.

| Case | Order | Evidence |
| --- | --- | --- |
| AR1 | Tool call, delayed review, Allow | `app-review.mjs` review |
| AR2 | Message, delayed review, Allow | `app-review.mjs` message |
| AR3 | Tool or message, answer another conversation/turn/review, Deny original with SDK `mcp_approval_denied` outcome | `app-review.mjs` target |
| AR4 | Context request without a review | `app-review.mjs` message |
| AR5 | Two answer callbacks overlap; another call becomes last | `fake-gateway.test.ts` captured callback |
| AR6 | SDK signature changes or four raw arguments return | `fake-gateway.test.ts` type assertions and fixture typecheck |

The fixture matches the review it owns and offered choice to settle its simulated
request. It is not a second production authorization decision. Server authority,
permission-target issuance, and app routing stay in their existing owners.
