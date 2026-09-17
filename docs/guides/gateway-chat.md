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
The panel connects to `ws://127.0.0.1:7420/session`. This authenticates local access;
remote TLS/device provisioning is not included in this delivery.

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
input cannot be approved through a truncated view. Uploaded panel attachments
remain unsupported by this text-only contract and stay in the draft with an error.

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
provider owns its context management; input text has a separate 8 KiB UTF-8 limit.

The initial gateway retains up to 32 conversation owners per server instance,
including closed ones. It reports capacity before persisting rejected creates.
Failed initialization remains unavailable until server restart; shutdown retries
any retained cleanup handle. These limits avoid silently replacing an owner whose
cleanup is uncertain. No automatic retry is exposed for permission/close controls:
an unknown control acknowledgement requires a refreshed view and a deliberate new
action, so an old Close cannot stop newer work.

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
atomic rename. The service's executable, `--desktop-runtime` argument and runtime
`PATH` point exclusively at that retained version, so replacing `Nessa.app` cannot
replace files under a running gateway. Corrupt existing versions cause an error;
they are never repaired in place. All published versions are retained; automatic
runtime garbage collection is not implemented. Failed attempts remove only their
own temporary directory. macOS may retain its protected `com.apple.provenance`
marker on both copy paths; runtime identity and policy do not derive from that
platform-managed attribute.

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
The service uses port 7420; a separately started development gateway must be stopped
once before enabling the installed service. Its working directory is `~/.nessa` and its log is `~/.nessa/logs/gateway.log`. User credentials stay under `~/.nessa` (stage and instance
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
