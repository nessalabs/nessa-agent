# Desktop question flights

One composed workspace store owns the asks already on their way
(`src/desktop/workspace/application/question-flights.ts`). The overview can
draw a question while the pane beneath it still holds the same card. Both
send through that store's record. Two stores do not share one.

The key is the question's identity: session, execution, and question id.
The stamp is the choices and the initiator, in the order the command
received them. The same stamp joins the send already on its way. A different
stamp does not start, does not replace that send, and does not receive its
outcome.

`start` runs before the key is recorded. A `start` that joined the same key
again would start a second send. `answerQuestion` does not do that: it asks
the source and returns.

## Orderings

| State | Event | Result | Evidence |
| --- | --- | --- | --- |
| Idle | `join` with a stamp | `start` runs now. The record holds that stamp and its promise. | `question-flights.test.ts` › lets a second caller hear the refusal, then start again |
| In flight | `join` with the same stamp | `start` does not run. The caller receives that same promise, including a refusal. | same test, the second join (`started` stays 1) |
| In flight | The promise settles | The key is cleared only when the record still holds that promise. | same test, the join after `release` starts again (`started` becomes 2) |
| Idle again | `join` after settlement | A new `start` runs. A refused send can be tried again. | same test, the join that resolves `"sent"` |
| In flight | `join` with a different stamp | Resolves `differentFlight` immediately. `start` does not run. The send already on its way is left alone and is not this caller's outcome. | `question-flights.test.ts` › does not hand a different answer the outcome already on its way |
| Settled after a different stamp waited | `join` with the stamp that was refused | A new `start` runs. The earlier different stamp did not take the first outcome. | same test, the join after `release` |
| In flight in one store | The same key and stamp on another store's record | That record starts its own send. | `question-flights.test.ts` › keeps two composed records apart |
| Question off screen, or the transcript no longer lists it | `answerQuestion` | `not-asked`. No send starts. The card leaves; that is not "already answered". | `transcript.test.tsx` › removes a question card the window is not showing, without saying it was answered; and the missed dispatch in answers a question the conversation is showing and leaves one it is not |
| On screen, listed, idle | `answerQuestion` | One send starts. | `transcript.test.tsx` › does not send a question again while the first answer is on its way |
| In flight | `answerQuestion` with the same choices and initiator | Joins. One send. Both hear how it settled. | same test, the second dispatch; `transcript.test.tsx` › lets a second card retry after the send already on its way is refused |
| In flight | `answerQuestion` with different choices, or a different initiator | `different` immediately. One send, the first. The second card stays usable and says a different answer is already on its way. | `transcript.test.tsx` › does not report a different answer as sent while one is already on its way; and keeps a card whose answer differs from the one already on its way |
| Settled as refused or unknown | `answerQuestion` again | A new send starts. | the retry in lets a second card retry after the send already on its way is refused; and the answer after the refused skip in does not report a different answer as sent while one is already on its way |
| In flight in one store | The same answer in another store | Two sends. | `transcript.test.tsx` › does not share a question send across two stores |

A card that heard `sent` stays busy until a later view drops the question.
That busy flag is the card's, not the flight's: the flight clears when the
send settles. `sent` is covered by does not send a question again while the
first answer is on its way, and the card leaving by answers a question the
conversation is showing and leaves one it is not.
