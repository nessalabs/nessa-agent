# Pre-save revoked explanation and the retained-read import (#860, #861)

Two small corrections. Neither adds a state, a second decision about access, or a compatibility path.

## #860 — what a caller is told

`NessaConversationMutationError` already types a wire `revoked` and marks it certain (`uncertain` false), because `rejectedBeforeDispatch` says the gateway refused before dispatch. `REFUSALS` has sentences for `revoked_after_save` and `unavailable_after_save` and none for `revoked`, so the caller is told "Conversation command failed".

The sentence comes from the existing `REFUSALS` table, keyed by the typed code. The wire message is not shown. Whether the outcome is certain stays with `rejectedBeforeDispatch`.

### States

The client does not move a grant. It explains the code the gateway already decided.

| Code | Saved | Reached the agent | `uncertain` | Sentence |
| --- | --- | --- | --- | --- |
| `revoked` | no | no | false | Access changed before the message was admitted, so nothing was saved. Sending it again needs a new message. |
| `revoked_after_save` | yes | no | true | The message was saved, then refused because its access changed before it reached the agent. Sending it again needs a new message. |
| `unavailable_after_save` | yes | no | true | The message was saved, then could not be checked because its access record could not be read. Sending it again needs a new message. |
| unknown code, or a transport failure | unknown | unknown | true | Conversation command failed |

`NessaConversationControlError` reads the same table. A control that arrives with one of these three codes gets that row's sentence and the same `uncertain` value.

### Invariants

1. `REFUSALS` chooses the sentence. `rejectedBeforeDispatch` chooses certainty. The sentence does not decide certainty.
2. The three access sentences stay distinct. Each names its own timing.
3. `conversationId`, `requestId`, and `executionId` are the values the caller passed. `retry()` runs that same closure.
4. The wire text is not the sentence. The reproduction's cause message is `"access changed"`; the sentence is the row above.
5. The desktop reason map stays. `revoked` remains `invalid-request` there. That reason has no panel sentence of its own, so a panel that met this error would show the client sentence. There is one copy of the words.

The regression constructs both error classes from the package export. One assertion covers each row, including the unknown-code row.

### Alternatives

- Leave the generic sentence. The caller has a typed, certain refusal and no account of it. That is the defect in #860.
- Reuse the `revoked_after_save` sentence. It says the message was saved. This refusal saved nothing.
- Show the RPC message. Callers branch on the typed code. The wire text is not a sentence we wrote.
- Mark the post-save codes certain so the flag matches "we know it was saved". Certainty has one owner, `rejectedBeforeDispatch`, and this change leaves that owner alone.
- Write a panel sentence and leave the SDK generic. The defect is the SDK boundary. A second sentence would be a second owner of the same words.

## #861 — short name in the fixture

`native_peer_answering` annotates its preparation closure with `nessa_auth::adapters::pairing::PairingCryptoError`. The file already imports `NativeIdentity`, `NativeTransport`, and `OsEntropy` from that module. The type joins that import, and the annotation uses `PairingCryptoError`.

### Order that stays

| Step | Event | Outcome |
| --- | --- | --- |
| Preparation succeeds | Both identities are generated, then the peer starts | The peer runs with those inputs |
| Preparation fails | The closure returns `Err` | The peer is not started |
| The caller unwinds | `Peer`'s drop joins the thread | The peer is joined |
| Retained refresh, follow, and the public read | Each keeps `Duration::from_secs(2)` | Unchanged |

### Invariants

1. Preparation returns before `Peer::spawn`.
2. `NativeIdentity::generate` stays fallible through `?`.
3. The two-second budgets and the joined-peer drop stay as they are.
4. No alias. Nothing else in the file is named `PairingCryptoError`.

### Alternatives

- Leave the full path. Once the type is imported at the top of the file, the annotation uses the short name.
- A second `use` line for this type alone. Types from one module share one brace import.
- An alias. The short name does not collide.

## Review brief

Base: `7b429791ec294ead4952ec86dde67d284d5aa9e9` (`main`).
Head: the commits on `cursor/pre-save-revoked-explanation-554e` that implement this note. Review the clean tree at that head. Name any uncommitted diff explicitly.
Checkout: this repository.

Intended behavior:

- A pre-save `revoked` mutation error says that access changed before admission and that nothing was saved, with code `revoked`, `uncertain` false, and the caller's ids. The two post-save codes keep their sentences and stay uncertain. A control carrying the same code gets the same sentence.
- The retained-read fixture names `PairingCryptoError` by its short import. Preparation-before-peer, fallible startup, the two-second budgets, and joined-peer cleanup stay as they are.

Owned scope: the `REFUSALS` entry and its regression; the one import and annotation in `crates/nessa-client-core/tests/read_only_sync/gateway/session.rs`. Whether access holds stays with the gateway. Certainty stays with `rejectedBeforeDispatch`. The panel reason map stays.

Inspect the three sentences against the table, the export path of the test, that the wire message is not copied into `Error.message`, and that the fixture diff is the import and the annotation.

Exclusions: no desktop layout change, so no browser script. The owner panel does not send a Drive grant's command; where that error's panel reason has no sentence, the client sentence is what would show, and this note does not add a panel sentence. No conversation-stress step: nothing a person does in a conversation changes. No Cedar change: this does not decide whether access holds. No new assertion in the gateway seam test `every_refusal_this_gateway_states_is_understood_by_the_client`. The gateway refusal tests are confirmation that those codes still mean what this table says.

Standards: [AGENTS.md](../../AGENTS.md) and [CODING_STANDARDS.md](../../CODING_STANDARDS.md). Gates that bind: typed failures, a diff that matches this claim, a test per failure row, one owner of the sentence and one owner of certainty, and this table before the code. The simplest account is the sentence for the code the gateway already sent.

Agreement: the code, the sentence, and certainty are three facts. The gateway owns the code, `REFUSALS` owns the sentence, and `rejectedBeforeDispatch` owns certainty. A post-save code stays uncertain and still has its own sentence. Those two facts are related and are not the same fact.
