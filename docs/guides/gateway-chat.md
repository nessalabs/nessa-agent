# Chat through the local gateway

The gateway owns one SDK Agent per conversation. The floating panel uses
`NessaClient.conversation` over the existing authenticated `/session` connection.
Each request checks current membership and the `conversation.write` grant;
conversation metadata further limits access to its organization and creator.
The credential identity supplies the verified surface attribution. Client metadata
cannot impersonate another surface or principal.

```text
Panel Send -> NessaClient.conversation.send -> authorized gateway command
  -> shared ConversationService -> Agent.enqueue -> Claude ACP process
  <- bounded conversation.read view <- live SDK observations + saved history
```

Arrows show calls and returned views. Sockets do not own Agent lifetime. Closing a
panel tab detaches its view; Stop closes active and queued Agent work. Later input
can resume the saved provider context through the SDK's existing lifecycle.
The read view shows at most 24 recent messages, bounded text and tool summaries,
within a 60 KB encoded budget. It marks omissions; it is not a full-history export.
Queueing, steering, withdrawal, permission state and cleanup belong to the SDK.
The server does not implement another scheduler.

## Local setup

First provision the gateway and panel credential using [local auth](local-auth.md).
Use the same stage, data directory and instance for the server and native panel.
The panel connects to `ws://127.0.0.1:7421/session` in the dev stage — the stage
decides the port (`protocol/defaults/gateway-ports.json`), and the installed
app's `prod` service keeps 7420. This authenticates local access;
remote TLS/device provisioning is not included in this delivery.

From a checkout, `just server` writes this section for you
([scripts/dev-agent-config.mjs](../../scripts/dev-agent-config.mjs)) and leaves
an existing one alone; everything below is the deliberate path, and the contract
that script writes to.

Add `agent` to the private namespace `config.json` (beside `auth/`):

```json
{
  "agent": {
    "catalog": "/absolute/path/to/models.json",
    "node": "/absolute/path/to/node",
    "acpEntry": "/absolute/path/to/claude-acp/dist/index.js",
    "workspace": "/absolute/path/to/your/project",
    "model": "exact-model-id-from-catalog",
    "toolsEnabled": true,
    "mcpServers": [{
      "name": "nessa",
      "command": "/absolute/path/to/nessa-agent/target/debug/nessa-mcp",
      "args": ["--workspace", "/absolute/path/to/your/project", "--audit-directory", "/absolute/path/to/private/process-audit"]
    }],
    "contextTokens": 100000,
    "outputTokens": 4096
  }
}
```

The installed desktop supplies an agent automatically. Its unattached workspace is
`~/.nessa/workspaces/default`; its directories are created below the private Nessa
data root through verified parent handles, so a symlink cannot redirect that default
workspace. Conversation journals remain under
`~/.nessa/conversations/sessions`. A project path enters the provider configuration
only after the user explicitly attaches or configures that folder. Because macOS
protects folders such as Documents, configuring one of those paths can produce a
system access prompt.

Use the SDK's [Claude ACP harness setup](../../crates/nessa-sdk/README.md)
for the pinned process. Provider credentials stay in the server environment or
Claude's configured credential directory. Requests cannot supply executables,
workspaces, environment variables or tokens. Missing agent configuration keeps
authentication/health available and returns `agent_not_configured` for chat.
Invalid supplied configuration fails startup. Claude process supervision currently
requires Unix; there is no production test-provider fallback.

Build `cargo build -p nessa-server -p nessa-mcp` before starting the gateway.
`toolsEnabled: true` exposes Claude's native tool preset, including WebSearch and
WebFetch. All Nessa-owned tools are supplied through the configured MCP servers;
`nessa-mcp` currently supplies `shell`, backed by Shepherd. Server executables and
arguments come only from trusted local configuration and enter the provider
restoration fingerprint. There are no automatically discovered MCP servers.

Native Bash, BashOutput and KillShell are disabled so commands use the MCP shell.
EnterPlanMode/ExitPlanMode remain disabled because this client holds the provider
in its default permission mode. AskUserQuestion is unavailable until the client
supports the harness's form elicitation. The preset does not invent capabilities
that the selected model or ACP client lacks. Native tools use Claude's executor;
Shepherd owns only Nessa's shell commands, not Claude's internal implementation.

The shell tool returns a tracked command ID, process identity, bounded stdout and
stderr, exit status, cause, and cleanup confirmation. It runs one foreground
command at a time per MCP connection, defaults to 120 seconds (maximum 3600),
and cleans up background descendants when the command ends. Cancellation, stdin
EOF and SIGTERM retain the admitted task through cleanup and final audit delivery.
Private `process-audit` records retain admission, start and completion; these are
separate from Nessa's permission audit. MCP owner IDs identify server lifetimes,
not human callers; command IDs link results with process records. See the
[MCP server guide](../../crates/nessa-mcp/README.md) for limits and verification.

The panel presents the exact original tool input and offered choices. Oversized review
input cannot be approved through a truncated view.

## Images in a message (panel and client)

This section describes the panel and `@nessa/client` side of the product
contract's `attachment.begin`, `PUT /attachments`, and the `attachments` list on
`conversation.send` / `conversation.steer`. It claims nothing about the gateway
beyond that contract.

A message is text plus image references; bytes never ride in a conversation
command, whose socket message is capped at 64 KiB. The panel uploads at attach
time, not at send, and it uploads the original file:

```text
attach -> SHA-256 of the original -> conversation.create (idempotent)
       -> attachment.begin -> stored { digest, mimeType, size }
                           -> upload_required -> PUT /attachments with the ticket
                                              -> 200 { digest, mimeType, size }
send   -> conversation.send { text, attachments: [the returned references] }
```

- **The gateway normalizes; the panel does not.** Converting, scaling, and
  compressing an image to what the selected model takes happens on the gateway,
  so those limits live in one place. The panel has no canvas work and knows no
  per-image byte or pixel limit. The only byte limit it puts on a single file is
  the 64 MiB a file may be to attach and upload at all — enough for a camera RAW
  file. (The client also holds a message's images to the protocol schema's
  5242880-byte `ImageAttachment` bound; that is the contract's, not a model's.
  Every such bound — that one, the 10 images and 10 MiB a message carries, the
  64 MiB the upload path takes — is generated into the client from
  `protocol/product/v1.json`. The conversation model keeps its own constants,
  because it may not import a client SDK, and the gateway adapter's tests hold
  them to the generated ones.)
- **The returned reference is what a message names.** Both `begin` (when the
  conversation already holds the bytes) and the upload answer with the stored
  reference, which may differ from the file in digest, media type, and size — a
  HEIC, camera RAW, BMP, or very large PNG comes back as a smaller PNG or JPEG.
  The digest the panel computes only identifies the upload and is never sent in
  a message; hashing reads Web Crypto, so the function that does it is injected
  from composition like any other read from outside the process. Storage is media-agnostic and the client's `StoredAttachment` type
  says so; narrowing one to an image a message may name (`asImageAttachment`) is
  a separate step, taken in the panel's gateway adapter.
- **Any `image/*` file is uploaded**; whether the gateway can read it is the
  gateway's answer. A browser reports most camera RAW files, and some HEIC, with
  no type at all, so at attach time a known image extension (`.heic`, `.dng`,
  `.cr3`, `.nef`, `.arw`, `.tiff`, … — `declaredMediaType` in
  `src/conversation/model/attachments.ts`) declares the file an image. The
  gateway reads the real encoding from the bytes. Files that are not images are
  still preview-only: they are not uploaded, and sending refuses them with a
  reason. An image the webview cannot paint gets a labelled tile, in the composer
  and in the transcript, rather than a broken picture.
- **At most three uploads run at once per window.** The gateway takes four and
  holds each slot until the image is normalized, so a window that started every
  upload when a dozen images were dropped would have most refused. The rest wait
  at `not-started`, shown as waiting, and start as slots free. When the upload
  route still answers `temporarily_unavailable` — the one refusal that does not
  spend the ticket — the same ticket is offered again after 1, 2, then 4
  seconds, and after that the tile fails as `busy` with its retry.
- **A failed upload is marked on its tile, and said once, briefly.** The tile
  shows the failure and carries the full reason and its own retry. Above the
  composer the Nessa UI notification says only that an image did not upload and
  why, in one short sentence, with one Retry for every upload worth retrying. A
  file no message can carry, and an agent that takes no images, are said the same
  way. Nothing there names a file or quotes a limit.
- **Upload state is on the file.** Each draft file is `not-started`, `uploading`,
  `stored` (with the whole returned reference), or `failed` with a typed reason:

  | Reason | From | Retry offered |
  | --- | --- | --- |
  | `unreadable` | This window could not read or hash the bytes. | yes |
  | `unsupported-image` | `unsupported_image`, or a stored reference in an encoding no message names. | no |
  | `too-large` | `image_too_large` (it could not be brought under the model's limits), or a stored reference over the protocol's 5 MiB image bound. | no |
  | `image-input-unsupported` | `image_input_unsupported`: the agent's model takes no images. | no |
  | `busy` | `temporarily_unavailable` after the bounded retries; a refused `begin` as `attachment_capacity` or `temporarily_unavailable`. | yes |
  | `interrupted` | `upload_interrupted`, `attachment_not_kept`, `upload_timeout` (the gateway's 408, or the client's own three-minute deadline for a PUT that never answers), or an aborted request. | yes |
  | `unavailable` | No connection or answer; `ticket_invalid`; `storage_unavailable` / `attachment_storage_unavailable`; `audit_unavailable`; a refused `begin` as `agent_not_configured` or `conversation_not_found`. | yes |
  | `rejected` | `size_mismatch`, `digest_mismatch`, a `begin` refused as `invalid_request` or with a code the client was not taught, an unrecognised answer, a stored reference malformed in some other way. | yes |

  A stored file that is no image a message can name is told apart by which fact
  made it one, because the tile says something different for each: the encoding
  (`unsupported-image`) and the size (`too-large`) are separate questions, so a
  readable 6 MiB PNG is not reported as a format the gateway could not read.

  The tile shows the state and says why. Removing a tile mid-upload is allowed;
  the late result finds no file and changes nothing. The composer's own copy of
  the draft never decides a file's fate: `setDraft` takes prose from the caller
  and files from the store, so a stale render cannot undo an upload, a removal,
  or a send.
- **`sendDraft` is the one place a draft is declined locally, always with a typed
  kind and a visible reason, and the draft is kept.** `not-connected` (no session
  yet: "Not connected to the gateway yet. Your draft has been kept."),
  `empty-draft` (the one silent kind: there is nothing to say about nothing),
  `message-too-large`, and for files `unsupported-file` (not an image),
  `upload-failed`, `upload-in-flight`, `too-many-images` (more than 10),
  `images-too-large` (more than 10 MiB together — both counted over the returned
  references, not the attached files), `image-input-unsupported`,
  `image-input-unknown`, and `unknown-attachment`. A message of images alone
  sends; its tab is titled by the first image's name. The composer's submit has
  no early return of its own except an attachment still being read, which says
  so in the panel.
- **Whether the agent takes images is never guessed.** `capabilities.imageInput`
  is false until an agent is open for the conversation. Staging an image creates
  the conversation and starts its reads, so the answer has normally arrived
  before anybody presses send; the composer also says so as soon as it is known.
  If no view has arrived yet, send is declined as `image-input-unknown`, a read
  is requested, and sending again a moment later goes through.
- **A send the gateway refuses before admission is not "delivery unknown".** The
  client knows which RPC codes the gateway decides before it admits a message
  (`invalid_request`, `agent_not_configured`, `agent_startup_deadline`,
  `image_input_unsupported`, `attachment_not_found`, `attachment_unavailable`,
  `conversation_not_found`, `conversation_capacity`): those leave `uncertain`
  false, and the code itself is reported as a typed `ConversationErrorCode`.
  `adapters/gateway/effects.ts` is where the panel reads that code, and it
  answers in the panel's own vocabulary — a `CommandFailure` such as
  `agent-startup-deadline`. It is the only place a wire code is read, for
  commands and reads alike; nothing anywhere compares an error's text against
  one. The panel marks the turn not sent, says why in a sentence chosen by that
  reason, and puts the message back in the draft with its images. A code this
  build has no word for keeps the client's own sentence and carries no reason at
  all, rather than being read as one it does know. Any other failure after
  admission was attempted stays uncertain and keeps its explicit retry.
- **A failed read has its own vocabulary, because a read is not a command.**
  Nothing was asked for and nothing changed, so there is no receipt, no draft to
  hand back and no outcome to be certain about — only a transcript older than the
  gateway's. `ReadFailure` is two words, and a word earns its place by changing
  what is said or whether a retry is offered. `configuration-changed` is the one
  the gateway will not serve again, so it withdraws the retry; `unavailable` is
  every other failed read, including every code this build has never heard of,
  and claims only that the view is stale and the panel is still asking. Every
  rejected read arrives as a `ConversationReadFailedError`, so the store never
  sees a wire code or a sentence, and `readError` on the tab holds the word
  rather than text. The cause is not thrown away with the translation: the store
  reports it to the console beside the word, which is the only place a gateway
  answering about a *different* conversation can still be read.
- **No read failure promises that the gateway will come back.** A third word for
  a transient outage was written and withdrawn, because no code the gateway sends
  means that. `temporarily_unavailable` and `agent_startup_deadline` normally do
  mean "not yet" — but when a failed launch cannot be confirmed stopped,
  `ConversationService` deliberately retains the conversation's slot, answers
  every later read from the cached failure and never attempts the provider again
  (`a_startup_deadline_with_unconfirmed_cleanup_retains_its_slot` asserts it, and
  the protocol text says "a launch whose process could not be confirmed stopped
  keeps that conversation blocked"). An adapter panic during opening reaches
  `temporarily_unavailable` the same way. The gateway cannot tell the two apart
  in the code it sends, so neither can the panel, and "this normally catches up
  in a moment" would be a confident falsehood for a conversation that is blocked
  until the gateway restarts. `unavailable` already says the panel keeps trying,
  which is true of all of them.
- **A conversation the gateway has stopped serving outranks everything else on
  its tab.** A command somebody asked for otherwise wins the notice over the
  panel's own polling. The exception is `configuration-changed`, because every
  other notice ends in an action — refresh, resend, retry this submission — that
  the gateway can no longer complete; a lost close acknowledgement inviting a
  refresh is the concrete case. What became of each message is not lost with the
  notice slot: the turn keeps its own receipt, which the transcript renders.
- **A control's failure is translated the same way, and says something else.**
  The client has no sentence of its own for a control: every one of them gets
  the constant "Conversation control did not return a trustworthy
  acknowledgement", which names neither the command nor its cause and is only
  true when the control really may have been applied. So the panel speaks where
  that would be false. `attachment_cleanup_unavailable` — the single image code
  that is not a refusal, where the close did happen and only its release of the
  conversation's uploads did not — says so, and changes nothing the panel does:
  a draft's stored images are forgotten after any close, acknowledged or not.
  Which sentence is shown is decided by what became of the control, never by
  the reason: refused says nothing was done, applied says the choice was
  recorded, and only a genuinely open outcome keeps the client's constant.
- **A permission answer reports the review's state, and it is authoritative.** A
  failed `conversation.answer` carries `selectionState`, which the protocol
  calls authoritative knowledge of whether the option was selected and
  independent of the diagnostic code beside it. The gateway sends an ordinary
  code there — its own test pairs a pending review with `audit_unavailable` —
  so a review left `pending` is certainly not applied under codes this build
  has no word for, and the outcome is carried even when the reason cannot be.
  A `consumed` review is the opposite certainty: the choice took effect and the
  command failed after it, which the client can only report as uncertain, so
  the selection state is read before the code rather than after it.
- **A control carries its reason and its outcome as two facts.** A reason never
  says whether the command ran: a control refused as `conversation_not_found`
  and one whose acknowledgement was lost carry the same word. `CommandFailure`
  is only what the panel *says*; `SubmissionRefusedError` and
  `ControlFailedError.refused` are what it may *decide* from.
- **Nor is a message the client would not put on the wire.** The client is the
  one boundary that validates a message's images — the panel's model puts no
  byte bound on a stored reference, because how heavy one image may be is the
  protocol's rule — and it refuses the arguments before anything is sent. That
  is as certain as a refusal gets, so it arrives as `invalid-request` with the
  client's own sentence rather than as a lost acknowledgement; the draft comes
  back. The scenario backend asks the client the same question, so a local
  session refuses what the gateway would.
- **A dead reference is uploaded again.** Closing a conversation on the gateway
  — which is what Stop does — releases every file it held, sent or not. So after
  Stop, every `stored` image still in that conversation's draft goes back to
  `not-started` and uploads again before the next send; this happens whether the
  close was acknowledged or not, because uploading bytes the conversation still
  holds answers `stored` without sending them. And when a send is refused as
  `attachment_not_found` or `attachment_unavailable` anyway, the recovered
  draft's images come back as `not-started` rather than offering the same dead
  reference again.
- **Retry re-sends the same references.** A turn's content does not change after
  it is sent, and the client freezes the list with the command, so one execution
  ID always names one message.
- **The upload request carries the ticket and nothing else**: no cookies, and a
  redirect is an error. It goes to the session URL's host and port over
  `http`/`https`; the browser preview's dev server forwards `/attachments`. The
  ticket is a secret and appears in no error message.
- **Sent images in the transcript.** A turn sent from this window paints its
  tiles from the local object URL — the original as attached, not the stored
  copy. Once the gateway has the message those originals are only previews: they
  stop counting against what may be attached, and at most 64 MiB of them are
  kept per window, the oldest falling back to the labelled placeholder (media
  type and size) that a turn known only from a view gets — after a reload, or
  sent from another surface. A message still sending, of unknown delivery, or
  refused keeps its originals, because it may return to the draft. Reading image
  bytes back from the gateway is not implemented.
- **Closing a tab.** A tab whose gateway conversation this window created only to
  upload into — a view has shown it empty and idle — is closed on the gateway
  when the tab closes, which releases its staged bytes; a failure there is
  logged and the tab still closes. Such a conversation is also not saved with
  the browser's tabs once it is known to be empty with nothing drafted. A tab
  with turns, or whose view has not arrived, is only closed locally, as before.

The preview budgets are 20 files, 64 MiB each, 128 MiB per draft, and 256 MiB
per window (sent originals excluded, as above). They are separate from the
message rules. Every size the panel shows is in binary units and says so (MiB,
KiB).

### Known limitations

- **No way to release one staged file.** Removing a stored tile is local: the
  gateway has no "release this hold" command, so the bytes stay held until the
  conversation closes. This needs a gateway API; it is not worked around here.
- **Closing a tab with turns does not release staged-but-unsent bytes**, for the
  same reason, and because closing a tab never stops a conversation's work.
- **A tab closed before its first view arrives** is not closed on the gateway
  even if this window created it, because it cannot be told from a restored
  conversation with a history.
- **An upload in flight when Stop is pressed** settles against the closed
  conversation's next opening; whether the gateway keeps that hold is its
  decision, and a send that finds it gone recovers as above.
- **Removing a tile does not abort its PUT**; the request finishes and its result
  is ignored.

## Views and retry behavior

The panel periodically reads a bounded current view while its conversation is
visible. This delivers live output without saving every streaming chunk. Each read
replaces the prior projection; revision values are transient, not durable replay
cursors. Views explicitly mark omitted history. Disconnecting and reconnecting
never resubmits prompts to reconstruct a transcript.

Each read keeps local intent the gateway has not acknowledged yet, so a view
racing an admitted send never resends or loses it, and local failures stay
visible. A queued or accepted receipt is the gateway's own answer about its
queue, not local intent: when a complete queue (`queueComplete: true`) omits that
identity and the bounded message view no longer carries it, the panel drops that
row instead of counting it as active work forever. The row is omitted history,
which the same view already marks with `truncated`; its outcome and audit remain
on the gateway, and the panel does not invent a terminal result for it. An
incomplete queue proves nothing and keeps those rows.

The client allocates stable conversation, execution and action IDs. Conversation
IDs are canonical lowercase hyphenated UUIDs. Execution and action IDs are limited
to 256 UTF-8 bytes. An uncertain
submission can be retried with the same immutable input and identities; the SDK
recovers its saved receipt instead of running it twice. Changed input is a new
submission, not an edit to a running operation. Pending input can be removed.

The gateway stores conversation ownership separately from SDK JSONL sessions.
A conversation's owner record is written and synced under a private temporary
name and only then published under its conversation ID, and publication never
replaces a name another owner already holds. An interrupted creation therefore
leaves either no record, so the same ID can still be created, or a complete
record whose original creator owns it. On Unix an interruption between
publishing the record and releasing the writer's own name leaves two links to
that complete record, which fails private-file verification until the next
gateway start releases the leftover temporary. A genuinely corrupt owner record
still fails closed and is never repaired; recovering that conversation ID
requires archiving the offending file outside the running gateway.

Opening a conversation's provider for the first time is gated on its mandatory
creation audit, whatever the entry point: if that audit failed, ownership
remains and read and send also refuse until the original creation evidence is
acknowledged. Repeating a create for an existing conversation acknowledges that
original creation first, then attributes the reopen to its caller, and only then
opens the provider, so a refused attribution leaves nothing reopened. A stored
creation-audit record that contradicts the owner record is a fail-closed state:
every operation on that conversation keeps returning an audit failure until that
record is archived and the evidence is rewritten from the owner record.

Consequential SDK boundaries persist accumulated output; a crash can lose text
from an unfinished stream. After a missed live observation the bounded view
fences ambiguous streaming text, which has no durable cursor, but text the
committed snapshot proves belongs to a settled message is rebuilt exactly once.
Mandatory execution/permission audit is independent of
views: private atomic JSON records are synced before acknowledgement. Those records
retain target, transition, cause, known actor, original input and local delivery
stage. Local closure and a written permission answer do not claim external tool
rollback or provider acknowledgement. Audit failure remains visible while cleanup
continues.

This initial bounded-view API does not require a second durable event database.
The exact-cursor replay and broader cross-principal collaboration proposed in
ADRs 0009/0011 remain separate future work.

The host reserves the effective input window minus the configured output allowance
for each submission, consistently across retries. This is a pessimistic admission
reservation, not token billing or a claim to measure opaque provider history. The
provider owns its context management; input text has a separate 8 KiB UTF-8 limit,
and may be blank only when the message carries an image.

The initial gateway retains up to 32 conversation owners per server instance,
including closed ones. It reports capacity before persisting rejected creates.
Failed initialization remains unavailable until server restart; shutdown retries
any retained cleanup handle. These limits avoid silently replacing an owner whose
cleanup is uncertain. No automatic retry is exposed for permission/close controls:
an unknown control acknowledgement requires a refreshed view and a deliberate new
action, so an old Close cannot stop newer work.

## Uploading attachments

A message names an image by reference; the bytes are uploaded first, on their own
path, so the 64 KiB product socket never carries them.

1. `attachment.begin` over the authenticated socket (grant `conversation.write`)
   describes the file: `conversationId`, `requestId`, `digest`
   (`sha256:<64 lowercase hex>`), `mimeType`, `size`. If this conversation already
   uploaded exactly that file the answer is `state: "stored"` with the stored
   `digest`, `mimeType` and `size`, and nothing needs sending. Otherwise it is
   `state: "upload_required"` with a `ticket` and `expiresAtMs`. The fields that do
   not apply are `null`: a stored answer never carries a ticket, and a ticket never
   carries a reference. One request holds one ticket: repeating `attachment.begin`
   with the same `requestId`, conversation and file replaces the earlier ticket,
   which stops working. The gateway keeps only a fingerprint of a ticket, so it
   cannot hand the first one out again; a caller repeats a `begin` because the
   answer never reached it, and then nobody ever knew the first ticket.
2. `PUT /attachments` sends the bytes as the request body with the ticket in the
   `x-nessa-upload-ticket` header. The route authenticates nobody: the ticket is
   the whole authority. It is single use and is spent the moment it is presented,
   whatever happens next, so a failed upload begins again with `attachment.begin`.
3. `200` answers `{"digest", "mimeType", "size"}`: the **stored** reference, which
   is what `conversation.send` and `conversation.steer` must name. The gateway
   normalizes an image after verifying the transfer, so the stored digest, type
   and size can all differ from what was sent. Any other file is kept as sent.
   Normalizing fits the image to the selected model's `imageInput` limits in the
   model catalog: it is converted to PNG or JPEG when the model does not take its
   encoding, turned upright, scaled to the long edge worth sending, and compressed
   under the byte limit. An image already inside every limit is kept byte for
   byte. The encoding is read from the bytes, not from `mimeType`. HEIC, AVIF and
   camera RAW are read where the operating system provides a decoder, which today
   is macOS; a client may declare them as `image/x-adobe-dng`, `image/x-canon-cr3`
   and the like, and anything declared `image/*` is normalized.
   `unsupported_image` (415) means the bytes are not a readable image;
   `image_input_unsupported` (415) means nothing is wrong with the image, but the
   selected model records no image limits and so is offered no images, which is
   the same code `conversation.send` gives for the same fact; `image_too_large`
   (413) means no legible version fits. What the normalizer answers is kept only
   if a message could name it: one of the four encodings below, at most 5 MiB.

Limits: a ticket lives five minutes and is refused after `expiresAtMs`; at most 64
are outstanding at once, 32 for one organization and 16 for one conversation,
and tickets whose time has passed are cleared out by every `begin` and every
upload, whoever makes it; a file is 1 byte to 64 MiB, which is what the image
library reads, so a camera's raw file fits; four transfers run at once, across
every caller (this gateway serves one organization, and a caller refused for
want of a slot keeps its ticket); two images are normalized at once, because
each holds a whole upload and its pixels in memory while transfers stream to
disk; one transfer has 120 seconds. One audit record has 5 seconds to be
acknowledged and one phase of them — a release's withdrawals, releases and
removals, or one sweep of expired tickets — has 30 seconds together, because
those lists are as long as a conversation has holds or the book has tickets;
records the budget does not reach are reported as lost evidence and the cleanup
they describe still happens. Storage takes any media type. What a message may
refer to is narrower: PNG, JPEG, GIF or WebP, at most 5 MiB each, 10 images and
10 MiB in one message.

A present `Origin` must be one the gateway trusts for `/session`, and is echoed in
`Access-Control-Allow-Origin` (never `*`); `OPTIONS /attachments` allows `PUT` with
`content-type` and `x-nessa-upload-ticket`. Any other origin is `403`.

`attachment.begin` fails with `invalid_request`, `conversation_not_found` (also
for another owner's conversation), `image_input_unsupported`,
`attachment_capacity`, `attachment_storage_unavailable`, `audit_unavailable`,
`temporarily_unavailable`, or `agent_not_configured`.
`image_input_unsupported` answers a `mimeType` of `image/*` on a gateway whose
selected model records no image limits and so is offered no images: no ticket is
issued, because no message could ever name what was uploaded. It is the same
code `conversation.send` and the upload route give for the same fact. The upload
fails with `{"code": …}`:

| Status | `code` | Meaning |
| --- | --- | --- |
| 401 | `ticket_invalid` | Missing, malformed, repeated, unknown, used, expired, replaced, or withdrawn ticket. |
| 400 | `size_mismatch` | More or fewer bytes than described, or a `Content-Length` that already disagrees. A long body is cut off at the first byte too many. |
| 422 | `digest_mismatch` | The right number of bytes, hashing to something else. |
| 400 | `upload_interrupted` | The body ended early. |
| 408 | `upload_timeout` | The transfer did not finish in time. |
| 415 | `unsupported_image` | Declared an image, but not one the gateway can decode. |
| 415 | `image_input_unsupported` | The selected model is offered no images. Nothing is wrong with the upload. |
| 413 | `image_too_large` | An image that cannot be brought under the selected model's limits. |
| 503 | `storage_unavailable` | Storing or normalizing failed, or no agent is configured. |
| 503 | `upload_unresolved` | The upload's own work stopped without an answer. Its ticket is spent and nothing it wrote was kept. |
| 503 | `audit_unavailable` | The upload was good but could not be recorded, so it was not kept. |
| 409 | `attachment_not_kept` | The upload was good, but its conversation let go of its files before it was kept. Begin again. |
| 503 | `temporarily_unavailable` | Too many uploads in progress. The ticket was **not** spent; retry with it. |

Every row except the last and the unknown-ticket cases spends the ticket. A refusal
that could not itself be recorded adds `"audit": "unavailable"` beside its `code`.

Every answer, a refused origin included, carries `Vary: origin`. The audit trail
names a refusal by the same word as its `code` above.

A hold is one conversation keeping one stored file, digest and media type
together. The same bytes kept as two types are two holds on one copy, so
declaring a file again as something else never takes away an image a sent turn
names; uploading a file the conversation already keeps changes nothing and is
recorded as already held. A hold is written pending, its creation is recorded,
and only then does it become usable: `attachment.begin` never answers `stored`,
and `conversation.send` never accepts a reference, for a hold whose evidence is
not committed. An upload that fails after that point takes back its own pending
hold and no other, and the trail says so (`attachment_hold_reverted`).

A conversation holds what was uploaded into it until it closes. Closing withdraws
its unused tickets, releases its holds, pending ones included, and removes bytes
nothing else holds. Who is closing comes from the ownership record, before
anything else, so a stranger's close and a close of nothing let go of nothing.

The files go once the agent is known to be closed, or once its saved session is
known to hold no unsettled turn that names images. A close that could not reach
the agent (no room for another live conversation, a provider that will not
start) reports that failure and keeps the files: an agent that was never closed
keeps its queued turns, and a queued turn that names images reads their bytes
when it is dispatched. Closing again, once the agent can be opened, lets them go.

If the cleanup or its audit record fails, `conversation.close` answers
`attachment_cleanup_unavailable` when files are still in place, and
`audit_unavailable` when everything went and only the evidence of it was lost;
the conversation is closed either way. If the agent failed as well, the agent's
code is the one answered and both failures are kept in the gateway's own error
and log. Closing is not final: a conversation can be reopened and nothing in its
ownership record says it was closed, so uploads after a close are held like any
others until the next close. A retry of a turn the agent already has does not
depend on its upload still being held: the same images recover the original
delivery and any others are `submission_conflict`. An upload that was never sent
is not expired on its own. Every transition (ticket issued, replaced, expired or
withdrawn; upload refused; hold created, already held, reverted or released;
bytes removed) is committed to private audit storage under `attachments/audit/`
with its target, both digests, cause, initiator and correlation.

Known limits. A ticket outlives the revocation of the credential that was given
it, for at most its five minutes: the upload route authenticates nobody, and
re-deciding access there would be a second, weaker copy of the socket's
authorization rather than the same one. What such a ticket can do is fixed when
it is issued: put one described file into one conversation of its own caller.
The provider limits one request to 32 MB, and an agent that resends earlier
turns' images can reach that in a long, image-heavy conversation. Stored images
are typically a few hundred KB at the 2000 px target, which is why this is a
limitation today and not a guard. A pending hold left by a crash stays invisible
until the same file is uploaded again or its conversation closes.

## Reorder waiting messages

Drag a waiting message using its queue handle, or focus the handle and use
Space, Arrow Up/Down, then Space. `NessaClient.conversation.reorder` sends the
complete desired list of pending execution IDs. The gateway calls the SDK's
`Agent::reorder_queued`; it never removes and resubmits messages. The SDK preserves
IDs, content, receipts and steering priority and serializes the move with dispatch
and cancellation. Steering messages must remain ahead of ordinary queued work.

A changed queue returns `queue_changed`; crossing the priority boundary returns
`priority_conflict`. The panel refreshes the current view instead of guessing or
replaying. An uncertain control acknowledgement also triggers a read. Reorder is
unavailable when the view omits pending IDs (`queueComplete: false`) or while
another control is pending. Older transcript truncation alone does not disable it.

Successful changes retain session-level before/after order and verified actor
alongside invocation history. Storage failure can leave a move applied in memory;
the failure remains visible and retained evidence is saved before later dispatch.
Restoration validates this evidence but never replays unfinished queued messages.

## Panel connection recovery

The panel continues retrying temporary connection failures after the client's
limited retry window ends. It maintains one connection attempt and releases stale
clients when the panel lifecycle ends. Authentication failures require an explicit
retry after fixing credentials. Reconnecting only establishes transport; it never
replays conversation commands.

Initial gateway preparation uses the empty-state status without claiming that a
prior connection was lost. Once a session has connected, transport recovery and
connection or submission errors use the Nessa UI notification above the composer.
Its retry action reconnects while offline, or retries the original
submission identity when delivery is uncertain. An input that never reached the
message API is marked unsent and stays in the draft. A rejected follow-up does not
settle earlier work. Stop is unavailable while disconnected because it requires a
gateway acknowledgement. The composer's generating animation is disabled.

### Restoring after local configuration changes

Saved Claude sessions retain the exact context fingerprint, including workspace,
launch arguments, system prompt, and tool/MCP configuration. A mismatch returns
`conversation_configuration_changed`; it does not start another context or erase
history. The gateway logs the underlying opening failure and retains any required
cleanup owner. Retrying the same configuration does not resolve a mismatch.
Start a new conversation using the current configuration. No stored-format migration
or older-version compatibility path is provided.

### Conversation activity surfaces

The panel groups adjacent tool calls behind a `Ran N tools` activity cue.
Opening it shows shared Nessa UI tool disclosures with provider output and any
arguments captured through permission review. Details are bounded and output
truncation is marked. Permission choices remain separate and require complete
input. ACP `agent_thought_chunk` text already follows the SDK message path; the
panel shows it behind a Thought sheet without inventing a duration. The gateway
supplies replacement views. The frontend agent-stream adapter maps each snapshot
into AgentEvent envelopes and a fresh TranscriptBuilder, then renders its Transcript
with the shared activity components. Repeated polls never append duplicate history.
`ConversationMessage.parts` is the output contract: text, thought and tool references
carry execution-local observation offsets. Text/thought fragments also preserve ACP
`messageId` when supplied. The adapter combines adjacent fragments only within the
same message identity and channel; distinct provider messages remain separate bubbles.
The gateway does not concatenate the execution into a single assistant answer.

Injected inputs retain `steeringTarget` and `steeringOffset`: the target's retained
event count at local admission, saved by the SDK before calling the provider. This
orders the user's message relative to observed output, not the provider's internal
consumption timing. Live and restored views use the same saved position. A provider
may answer several inputs in one message; no separate answers are invented. Without
a provider message identity, adjacent same-channel fragments form one segment.
The shared builder's extracted final text is rendered at its original event position,
so commentary before tools stays before tools, while final replies follow them.
Unknown timestamps and usage remain absent; raw DTOs retain partial tool output.
Local user attachments and delivery receipts stay correlated by the prompt event ID.

Waiting messages use the composer queue badge and sheet. Reorder, promote, and
remove retain execution identities; promotion preserves the steering priority
partition. The composer exposes Queue/Steer while work is active. Stop shows
Cancelling until acknowledgement, then Cancelled; failed acknowledgements do not
claim success. Tab context menus offer local-tab Rename and View details. Runtime
facts come from server composition (provider/model/workspace), with no timestamps
or unsupported sharing actions. Rename does not change gateway conversation IDs.

## Installed macOS runtime

The DMG contains `Nessa.app` with the gateway, `nessa-mcp` (including Shepherd),
an official Node runtime, the locked Claude ACP harness and its dependencies,
and the model catalog under `Contents/Resources/runtime`. Install the app in
Applications before launching it. No repository checkout, Cargo, npm, or Homebrew
Node is needed at runtime. Claude credentials remain in the user's local Claude
configuration; credentials and chat history are never shipped inside the app.

Before changing any service, the desktop stages its bundled runtime under
`~/Library/Application Support/Nessa/gateway-runtimes/<service-label>/<fingerprint>/`.
On APFS it copy-on-write clones regular-file data into a private temporary sibling;
other filesystems use the durable byte-copy fallback. It copies relative internal
symlinks, preserves bytes and executable bits, removes removable bundle-supplied
extended attributes, syncs every file after either copy path and then its directories, verifies
the same full-tree SHA-256 used by packaging, then publishes with an exclusive
atomic rename. The service's executable and `--desktop-runtime` argument point
exclusively at that retained version, so replacing `Nessa.app` cannot replace
files under a running gateway. No search path is derived from it: the gateway
runs `nessa`, `node`, the ACP entry and `nessa-mcp` by absolute path, so the
staged directory appears on nobody's `PATH`. Corrupt existing versions cause an error;
they are never repaired in place. Failed attempts remove only their
own temporary directory.

Once reconciliation has a gateway up, and while it still holds the per-label
lock, it collects the versions nothing can be running: everything under the
label except the version just registered, the version the loaded service reports
over `/health`, and any version named by an unanswered retirement request or by
a retirement fence that records cleanup or audit failure rather than a completed
retirement. An update therefore leaves one published version behind, or two
while a retirement is outstanding. A fence whose retirement was acknowledged
describes a gateway that cleaned up and was then booted out, so its version is
collected; honouring it forever would keep one stale generation for good. Interrupted staging attempts
are collected too, because holding the lock means no other host is staging.
Nothing else in the directory is touched: only a name this host writes — a
64-hex fingerprint, or `.staging-` and one — is ever a candidate, and each
removal is re-checked against the filesystem for a real directory this user
owns. A reconciliation that failed collects nothing, since its predecessor may
still be running. One entry never stops the pass: an entry the directory will
not yield, and one whose name is not valid text, are each reported and stepped
over, and the versions beside them are still collected. A removal that fails is logged with its path and changes
nothing about registration: a gateway that is up matters more than disk that was
not reclaimed. macOS may retain its protected `com.apple.provenance`
marker on both copy paths; runtime identity and policy do not derive from that
platform-managed attribute.

The service runs with the system `PATH` and nothing more. What the *agent* gets
is a separate variable, `NESSA_AGENT_PATH`: the desktop host asks the account's
login shell for its path once, while registering, and writes the answer into the
launchd definition. That makes it part of the service's identity — changing it is
a deliberate re-registration, not something that shifts under a running gateway —
and it is resolved from a clean login shell, so launching Nessa from a terminal
with an unusual path does not rewrite the service. It is asked once per run of the app, not once per
reconciliation: the panel reconciles on every webview load, and a profile edited
while Nessa is open would otherwise produce a different definition and retire a
healthy gateway mid-session. A changed profile therefore takes effect the next
time the app is launched, and that launch re-registers the service. The shell is asked
the ways that reach the files a user's tools are actually in: zsh once, as an
interactive login shell, which reads everything it has; bash twice, because no
single bash reads both `.bash_profile` and `.bashrc`, with the two answers
combined. And the
answer comes back between unguessable markers, so a profile that prints a banner
or tries to answer for the shell does neither. Each attempt is bounded by one
deadline covering output and exit together, and a shell that overruns it is
killed with its process group. The order when something goes wrong is
interactive login shell, then login shell, then the path already registered for
the service, then the system path; each step says so on stderr, and none of them
costs the registration anything. Nothing is asked again until the app is
launched again. The staged runtime is never on it, so the
agent's `node` is the user's or none at all.

The desktop bootstrap registers `so.nessa.gateway.prod` in the user's launchd
GUI domain. launchd starts the gateway on login and restarts unexpected exits.
Reopening the same runtime reuses the service only when its executable, working
directory, namespace, provider configuration, and runtime fingerprint all match.
The desktop persists a random 64-hex `NESSA_SERVICE_GENERATION` in each installed
launchd definition. It reuses that generation only when the complete definition,
excluding the generation field, still matches and no recorded retirement cause
fences its admission. Cleanup or audit failure does not reopen admission. Changed
or retired definitions receive fresh entropy, including a
return to previously used configuration. Failed bootstrap retries reuse the
unfenced desired generation already written to disk.
Health must advertise that service generation, a canonical runtime-instance UUID,
and a process ID matching the exact loaded launchd service. A managed update exchanges private correlated
requests/results, fences admission, and requires cleanup and audit acknowledgement
before unloading the old service. Requests/results carry the running and target
service generations as well as the process instance. Results distinguish the
requested running generation from the actual runtime; a pre-admission rejection
with no retirement cause never fences the actual generation. A matching published
request conservatively forces reconciliation when result publication is missing.
Admitted results retain the original validated principal, lifecycle cause, and
retirement-request UUID across restarts and repeated acknowledgements. The host syncs
the successful result and its directory before relying on the retirement fence. Stale or malformed results never
authorize replacement and are preserved while awaiting the fresh transaction.
The first update from a headerless legacy gateway uses an explicit bootout only
when its sole listening PID matches the loaded service PID; that path stops active
agents. Healthy existing direct-app registrations migrate through the same managed
or legacy path, then use a retained staged version. Missing or ambiguous process
identity preserves the service. The one-time limitation is an old direct-app
registration already left inactive by replacement before it adopted staging. The on-disk plist
cannot prove the configuration retained by launchd; diagnostic output is not a
supported full-definition API. This case requires explicit service management
rather than automatic bootout inferred from unavailable health or disk contents.
Installation failures preserve the desired definition and any loaded replacement
for forward recovery through Retry. Before asking launchd to bootstrap a new
definition, the host stores a private durable install-attempt record containing
the exact service, definition, fingerprint and generation. It removes that record
after verified readiness. If readiness fails, Retry may unload and bootstrap the
unambiguously PID-less unavailable
registration only while the record, current desired definition and complete plist
still agree under the same service-label lock. A missing, malformed or mismatched
record preserves the service, so the plist never grants replacement authority by
itself. A bootstrap failure clears the record only after launchd positively reports
the label unloaded. The host never restores an old plist.
Closing or quitting the desktop does not stop the service. The menu bar's
**Stop active agents when quitting** checkbox writes `stopAgentsOnQuit` to the
native `settings.json`; it defaults to `false`. When enabled, quitting sends an
OS-local signal to that launchd service to close agents while keeping gateway
admission open. An actual service shutdown flushes agent journals before exit.

First launch provisions private local credentials if absent. Existing credentials,
workspace/model settings and histories are preserved. Bundle-owned runtime paths
are supplied by composition, without writing installation paths into user config.
The installed service uses the `prod` port, 7420. A development gateway runs on
the dev stage's own port (7421), so the two coexist and neither has to be stopped
for the other. Its working directory is `~/.nessa` and its log is `~/.nessa/logs/gateway.log`. User credentials stay under `~/.nessa` (stage and instance
namespaces still apply). Custom user-supplied MCP servers are not packaged.

`pnpm app:build --bundles dmg` prepares the complete runtime, builds the disk
image, and verifies the final packaged runtime before returning success.
The preparation script verifies the official Node archive against its release
checksum and installs the ACP harness from its lockfile. Packaging currently targets
native macOS architecture and requires macOS 13.5 or newer at runtime.
Preparation recreates the runtime directory, then fingerprints every prepared
file, including the model catalog and installed ACP JavaScript dependencies.
The digest includes relative paths, executable permissions, and internal symlink
targets; it excludes timestamps, installation location, and the generated root
manifest. This identifies the prepared runtime inputs, not a code-signing trust
decision. External or broken runtime symlinks fail packaging.
Packaged web content uses a production CSP: scripts load only from the app,
native IPC and the numeric-loopback gateway are explicit connection targets,
and blob/data/HTTPS images needed by previews remain available. Development disables
the packaged CSP for Vite HMR; that development-only behavior is not shipped as a
production allowance.
To remove the background service, boot it out with
`launchctl bootout gui/$(id -u)/so.nessa.gateway.prod` and remove its plist from
`~/Library/LaunchAgents`. Do not delete user data to uninstall the service.
