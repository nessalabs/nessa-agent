# System instructions and user input

`SystemPromptBuilder` builds only `SystemPrompt`: reusable system instructions with
ordered source attribution. `ExecutionRequest` contains a separate `user_message: UserMessage`, an `ExecutionId`, and caller-supplied token admission counts. For example:

```rust
use nessa_sdk::domain::agent_execution::prompts::SystemPromptBuilder;
use nessa_sdk::domain::agent_execution::prompts::{PromptSource, PromptSourceKind};

let core = PromptSource::new(PromptSourceKind::Core, "system")?;
let plugin = PromptSource::new(PromptSourceKind::Plugin, "security-review")?;
// The host supplies the plugin's PLUGIN_PROMPT text after resolving it.
let plugin_prompt = "\n\nFocus on permission handling.";
let prompt = SystemPromptBuilder::new()
    .text(core, "Review the proposed change.")
    .text(plugin, plugin_prompt)
    .build()?;

let plugin = prompt.contributions().nth(1).unwrap();
assert_eq!(plugin.source().name(), "security-review");
assert_eq!(plugin.text(), plugin_prompt);
```

Source kinds are `Core`, `Plugin`, `Mcp`, `Skill`, and `User`; each source also
requires a nonblank name identifying the contributor. Contributions retain exact
text (including empty appends and whitespace) and insertion order. The caller
appends the core system prompt first, then plugin `PLUGIN_PROMPT` content and
other layers. The builder does not load plugins or sort by source kind. Provenance
is metadata, not a message role or a grant of authority. `contributions()` returns
an exact-size iterator of borrowed `PromptContributionView` values. Their text
slices reference the same backing returned by `text()`; the assembled prompt keeps
no second copy of its fragments. Empty contributions retain zero-length ranges.

Construction consumes the owned contributions while allocating and filling one
assembled buffer. Input fragments and output briefly overlap (up to twice the text
bytes, plus metadata), and fragments are dropped as they are copied. Blank-only
input is rejected before allocating assembled text. The finished value retains
text once; an explicit clone creates another independent backing. This removes
retained duplication without imposing a domain size cap or changing caller budget
responsibilities. `payload_bytes()` reports the retained text, contribution slots,
and source-name bytes without rendering or copying text; it excludes allocator
bookkeeping and the inline prompt value.

Configure these instructions using `ClaudeAcpProvider::with_system_prompt(prompt)`.
For the pinned Claude harness this replaces its default system prompt, using
`session/new._meta.systemPrompt`. Source metadata stays local and remains available
through `binding.system_prompt()`. If omitted, the harness uses its own default;
Nessa does not claim to capture those hidden instructions. Standard ACP has no
portable system-prompt setter, so this extension stays in `claude_acp`.

A `UserMessage` is one turn's `PromptText`, the images it carries, the files it
points at, or any combination of them. It refers to each image by
`ImageReference` (SHA-256 digest, one of four media types, and size) and never
holds bytes, so a request stays small enough to compare for retry identity,
queue, and persist whole. Its constructor owns the rules: at least one of the
three, at most 10 images, 5 MiB each and 10 MiB together, and at most 10 files.

A `LinkedFile` is the other half, and it is carried in the opposite way. It holds
one absolute path and nothing else — no digest, no media type, no size, because
nothing about the file travels and nothing here opens it. It becomes a
`resource_link` block, which every ACP agent accepts, and what the agent does
with one is put the text `[@name](file://…)` in front of the model; the model may
then read the file with its own file tool, or may not. Nothing waits to find out.
The name is the path's last component, derived rather than carried, so the two
cannot disagree about which file is meant.

Its constructor owns rules about what the path *means*, which is all that can
be known without opening anything: absolute, no control character, ending in a
file name, every component below the root a name rather than `.`, `..` or
nothing, and at most 4096 bytes. Each of those is a rule about a path that would
name a different file from the one chosen — a dot component is resolved away and
an empty one is dropped when a path is written as a URI — or, for a control
character, a path nobody can be shown before they approve the read. A path that
is not UTF-8 cannot be described at all, and whoever obtained it says so rather
than passing on a lossy rendering.

**No rule here is about markdown, and that is the point.** There used to be one:
square brackets were refused anywhere in a path, because the adapter writes the
file into the prompt as `[@name](uri)` and a bracket could close the label. It
was written twice and was wrong twice about which characters could do that —
`)` ends a destination at the first unmatched one, and `\` escapes whatever
follows it, so a name ending in a backslash ate the adapter's own `]` and turned
the whole attachment into prose carrying its own `file://` URI. Both were found
after shipping.

So the rule is gone rather than extended a third time, and the two strings the
adapter interpolates are constrained instead of the paths people may own. The
URI is percent-encoded down to the RFC 3986 unreserved set plus `/` and `%`; the
label backslash-escapes every ASCII punctuation character. Neither needs to know
markdown's grammar to be safe from it, which is the property a list of
metacharacters could not have. `prompt_link_attacks.rs` states the whole
guarantee — one link per file, each destination decoding to that file's path,
each label that file's name, and no text outside the links — and checks it over
every Unicode scalar with a CommonMark parser the crate did not write.

Nothing here is resolved, followed, or checked for existence. Whether the file is
there is only true or false when the agent opens it, which is later and — under a
tool policy that puts `Read` in `ask` — after a person has approved that read.
Linked files need no `UserImageSource`, no capability from the agent, and no
model limits: `resource_link` is one of the two baseline ACP content kinds.

Image input has four gates, and each can only narrow the one before it. The
model's metadata must list image input and record its `ImageInputLimits` (media
types, encoded size, and edge ceilings, in `data/models.json`); a model without
recorded limits is offered no images, because nothing could prepare one for it.
Those limits are read in two places. Whoever prepares an image reads them from
the selected model, `ModelMetadata::image_input()`: the gateway does this at
composition, before any agent is open, so preparation never depends on a live
session. Admission reads the same limits from the session's
`EffectiveCapabilities::image_input()`, which is present exactly when the model
and the binding both offer image input. The message's own ceilings stay absolute
whatever a model allows. The binding offers image input only when composition
gave `AcpConfig::images` a `UserImageSource`. And the connected agent must have
advertised `promptCapabilities.image` at `initialize`, reported as
`OperationCapabilities::image_input`.

Every one of those is checked when a message is submitted, before it is
accepted, by `Agent::invoke`, `enqueue`, `enqueue_steering`, and `steer` alike.
A refusal there saved nothing, queued nothing, and sent nothing:

| Refused because | Error |
| --- | --- |
| the model or the binding offers no image input | `ImageInputRefused(NotOffered)` |
| an image's encoding is not one the model lists | `ImageInputRefused(MediaType(..))` |
| an image is larger than the model's `max_raw_bytes()` | `ImageInputRefused(ImageTooLarge { .. })` |
| the agent is known not to take images | `ImageInputRefused(AgentDoesNotAccept)` |
| the encoded message cannot fit `max_frame_bytes` | `MessageTooLarge { .. }` |

`NotOffered` and `AgentDoesNotAccept` are two different facts and stay apart:
the first is this attachment, which was never going to carry an image; the
second is the agent now on the other end of it, which a restoration can change.

The agent's answer is refused only when it is a known "no".
`OperationCapabilities::negotiated` is false while a context is being opened or
restored, and then `image_input: false` means "not known yet": the message is
admitted, and the adapter answers the same `ImageInputRefused(AgentDoesNotAccept)`
at dispatch if the restored agent turns out not to take images. A closed context
keeps its last negotiation. The frame check counts the text as JSON, every image
as base64, and about 2 KiB for the request around them; the largest message the
domain allows (10 MiB of images with several mebibytes of text) does not fit the
largest frame, and is refused here rather than after it was accepted.

Just before dispatch the adapter reads each image through the source, checks its
length and then its digest against the reference, and sends it as a base64 ACP
`image` block after the text. A missing, unreadable, or substituted image is
`AgentError::UserImage` and nothing is sent. The read happens on the task that
submitted the message, never on the task that owns the agent process, so a
source that stalls delays one message and nothing else: the active execution
still settles, steering and permission answers still go through, and close
still completes and abandons the read (`Closed`). All the images of one message
are given ten seconds together, less when the execution timeout or the
five-second steering deadline is shorter, and ten seconds even when the
execution has no timeout. A read that runs out of time, or a source that
panics, is `UserImage(Unavailable)`; the context stays usable. A source must
not return more than `size + 1` bytes for a reference: the port hands back an
owned buffer, so only the implementation can stop a larger one being built.

Reading spends the operation's own deadline rather than adding to it. An
`execution_timeout` starts when the request is submitted and covers the read,
the prompt write, and the run; the five-second steering deadline starts when
`steer` is called and covers the read and the acknowledgement. What the worker
arms when it dispatches is what is left of that interval, not a fresh one.

Writing a frame to the agent may take one second plus one more for each whole
mebibyte of the frame, so a message carrying images is not failed by the bound
meant for small frames; an execution timeout still ends the write earlier. That
write allowance is the one thing added on top of a steering deadline, so a
steering call carrying a frame of *n* whole mebibytes is bounded by five
seconds plus *n*.

Encoded image bytes travel in the session's command queue, so one session holds
at most twice `max_frame_bytes` of them outside its worker. A message whose
images would pass that is `Busy` before a byte is read; since an admitted
message always fits one frame, a message alone can never be refused this way.

`max_frame_bytes` bounds what this host writes. What an agent may send it is
`max_incoming_frame_bytes`, a separate ceiling with the same range, because
that one is the buffer an agent subprocess can make the host allocate: raising
the frame size to carry images does not raise what an agent can demand.

Each `Agent::invoke(ExecutionRequest { user_message, .. })` sends only that new
user message on `session/prompt`. The ACP agent maintains history and emits the
assistant response; the client does not resend earlier messages or append assistant
messages itself. Create streamed output values with `MessageChunk::text(...)` or
`MessageChunk::thought(...)`. These immutable values preserve empty text and
whitespace, discard spare capacity, and expose `kind()`, `as_str()`, and
`payload_bytes()`; optional provider correlation uses a validated nonempty
`MessageId` of at most 256 UTF-8 bytes. They do not fabricate saved history or impose the application
chunk-size policy.
See [ACP prompt turns](https://agentclientprotocol.com/protocol/v1/prompt-turn).
