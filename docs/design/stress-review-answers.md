# Stress review answer evidence

The conversation stress check answers through the window. Its observer reads the
window's existing gateway socket; it sends no command and changes no deadline.
A control ACK means the requested change was applied or already acknowledged
(`ConversationMutationResult`), not that the provider completed its tool. The
existing final transcript and at-rest checks still own that later evidence.

The observer is installed before navigation and watches only `/session` on the
run's gateway. It retains only the selected review identities, option, command
identity, typed selection state and response classification, never complete socket
frames or tool input. Local UTC millisecond observation times distinguish the
click attempt, sent command and response; they are not provider effect times.
One answer scope is active at a time. Socket identity scopes response ids.
ACK and disappearance are observed concurrently within the existing 30-second
post-click wait, rather than granting a second settlement interval.

| Ordering | Required result | Regression evidence |
| --- | --- | --- |
| Exact answer sent, matching command ACK with applied=true, review removed | ACK confirmed; disappearance is checked separately | stress-review-answers.test.mjs: exact applied ACK |
| Review expires without a sent answer | No ACK; step fails even when review disappears | expiry without an answer |
| Answer sent, ACK lost, review removed | Unconfirmed; step fails without retrying a new command | lost ACK |
| Answer sent after expiry, refusal received | Refused; step fails and retains refusal code | refusal after expiry |
| Wrong conversation, execution, review, permission or option | Not this answer; cannot confirm it | mismatched target fields |
| Another socket reuses a response id | Not this command's ACK | socket identity |
| Response envelope names another socket request despite the exact command receipt | Not this command’s ACK; the genuine response remains accepted | response envelope identity |
| ACK names another command, or applied=false | Not an applied ACK | command identity and applied flag |
| Refusal carries a published selection-state tag, an unknown tag or no tag | Retain only the generated protocol’s published states; refusal remains independent | refusal selection evidence consumes the published state rule |
| Observer disposed while sockets remain | Listeners detached; no later confirmation | disposal |

Historical #1039 ACP evidence proves a deadline deny reached the provider. It has
no timestamps and cannot establish why the human allow did not settle first.
These controls reproduce the verifier's false success, not that runtime cause.

The current `waitingRow` step requires a plain session row under Needs you,
with no request body, answer buttons or form beside it. Clicking that row opens
the conversation, where `answerCard` observes the exact command ACK and review
removal described above. The answered row must then leave Needs you. Main owns
this presentation contract; the parent Stop integration retains it.

Earlier list-answer runs exercised compact and preview cards in the session list.
Those controls were removed by main and are no longer acceptance criteria for
`waitingRow`. Their historical evidence remains in the parent-stop evidence
folder. The original Terra raised-run failure (compact Allow assumed for a wrapped
MCP review) remains failure evidence, not a production visibility-policy defect.
The existing large-review step and its load are unchanged.
