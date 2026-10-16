# Phone send

Shared by the owner-phone gateway ([#811](https://github.com/nessalabs/nessa-agent/issues/811))
and the phone outbox ([#269](https://github.com/nessalabs/nessa-agent/issues/269)).
The wire is the current product socket. This page names the fields both sides
use and the five states the outbox renders from them.

## Call

The phone uses the retained session from
[#782](https://github.com/nessalabs/nessa-agent/issues/782): `ReaderAccess`,
`ReadStop`, and a budget, on the pinned product session
(`RetainedCache::read`). `conversation.send` and `conversation.receipt` are
requests on that session. The assistant reply arrives later as a
`conversation.view` on `conversation.subscribe`
(`RetainedCache::subscribe`). Cedar admits the session.

## Request

`conversation.send` takes `ConversationSendParams`.

| Field | Rule |
| --- | --- |
| `requestId` | The phone generates it. One command id for this principal. This is the idempotency key. |
| `executionId` | The phone generates it. The turn id, kept with the request. |
| `conversationId` | The owned conversation. An empty conversation is a valid target. |
| `text` | The message. Empty only when `attachments` holds an image. At most 8192 UTF-8 bytes. |
| `attachments` | Images already uploaded, in order. Each is `digest` (`sha256:` and 64 lowercase hex digits), `mimeType` (`image/png`, `image/jpeg`, `image/gif`, or `image/webp`), and `size` (1 through 5 MiB). The bytes travel on `attachment.begin` and `PUT /attachments`. |
| `files` | An empty list. |

## Answer

`conversation.send` returns `ConversationReceipt`: the same `executionId` and
`disposition` `queued`, `injected`, or `settled`. A refusal is a
`ConversationErrorCode` and carries no receipt.

## Lookup

After a lost reply, `conversation.receipt` reads the outcome. The params repeat
the send: `operation` `submit`, `conversationId`, `requestId`, `executionId`,
`text`, `attachments`, and `files`. The call writes nothing and does not
enqueue. The result is `ConversationReceiptResult`: `found`, and when `found`
is true, `stage` (`accepted`, `attempted`, `settled`) and, once settled,
`outcome`.

That lookup is the command receipt on main
([command creation](command-creation.md)). `codex/268-command-receipts` and
`codex/268-semantic-consumer` are not in main.

## States

| State | When |
| --- | --- |
| `accepted` | `disposition` is `queued`, or lookup `stage` is `accepted`. |
| `running` | `disposition` is `injected`. |
| `done` | `disposition` is `settled`, or lookup `stage` is `settled` and `outcome` is `dispatched`. The assistant text is the later `conversation.view` for `executionId`. |
| `failed` | The send returns a `ConversationErrorCode`, or lookup `stage` is `settled` and `outcome` is `no_effect`, `refused_after_save`, or `unavailable_after_save`. |
| `unknown` | The send answer was lost and lookup `found` is false; the lookup itself failed; lookup `stage` is `attempted`; or the send returned `submission_unresolved`. |

A submit lookup settles as `dispatched`, `no_effect`, `refused_after_save`, or `unavailable_after_save`. `withdrawn`, `cancelled`, and `already_final` are `conversation.stop` outcomes; a submit lookup does not return them.

`submission_unresolved` is a restored `Attempted` receipt
([command creation](command-creation.md), row M4). The effect may have
happened. The outbox keeps this `requestId` and does not send another.

## Replay

The same `requestId`, `executionId`, `conversationId`, `text`, and attachments
in the same order return the original receipt. The gateway does not enqueue
a second turn.

A different conversation, turn, text, attachment, or operation under that
`requestId` is `submission_conflict`. Nothing new is enqueued. The lookup
sends the same bytes; a different fingerprint is `submission_conflict`, and
`found` stays unset.
