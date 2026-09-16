# Nessa MCP server

All Nessa-provided tools are exposed through MCP. This binary hosts the stdio
transport; each tool owns its domain, application and infrastructure modules.
Claude-native tools stay in Claude. The first Nessa tool is `shell`.

```text
Claude ACP -> MCP tools/call -> shell application -> Shepherd
                                      |                |
                                 private audit    owned process scope
```

Arrows represent calls. Nessa's ACP permission UI approves the original tool
arguments before Claude sends the MCP request. The local stdio server is a trusted
host capability, not an independently authenticated public network endpoint.
Do not expose it through an unauthenticated proxy.

## Run

```sh
cargo build -p nessa-mcp
./target/debug/nessa-mcp --workspace /absolute/project --audit-directory /absolute/private/process-audit
```

Configure it as server `nessa` in the gateway's `agent.mcpServers`; see
[gateway setup](../../docs/guides/gateway-chat.md). Restart the gateway and start a
new conversation after changing its tool configuration. Context fingerprints
prevent silently restoring old conversations under a different tool policy.

`shell` accepts `command` and optional `timeoutSeconds` (1–3600, default 120).
Commands run in the configured workspace using `/bin/bash --noprofile --norc -c`.
They receive only the explicitly composed PATH, HOME, USER, LOGNAME, TMPDIR, LANG
and LC_ALL values. This is process ownership, not a filesystem or network sandbox.
The executable currently requires Unix, matching Nessa's ACP process adapter.

Each connection permits one active command. The result includes its command ID,
Shepherd scope/process IDs, OS PID, exit status, bounded stdout/stderr, dropped
byte count, execution cause, and independent cleanup/audit failure evidence.
Private synced records retain admission before spawning, start with process
identity, and completion after cleanup. Failed admission audit prevents spawn;
later audit failure still performs cleanup and is reported as an error.
The tool response's command ID links these records to one owned process operation.
Each record also retains the provider's JSON-RPC request identity together with a
Nessa-generated private-connection identity. MCP `_meta` is not accepted as actor
or correlation evidence. The initiator is therefore recorded precisely as the
configured provider tool call; this transport does not claim a human identity.
The SDK permission audit separately retains verified user attribution when the
host approves or denies the original tool request.

Output capture retains at most 64 KiB of queued bytes across both streams; overflow
is reported, not silently presented as complete output. Output is displayed with
UTF-8 replacement for invalid bytes. This is a bounded diagnostic record, not an
unlimited binary output archive. No background process is intentionally retained
past the end of a command. Read EOF, cancellation and SIGTERM stop owned work and
wait for final evidence. Unverified cleanup ends the MCP connection before any
new command can be admitted.

On macOS, Shepherd uses process groups and cannot contain deliberately detached
descendants. Abrupt SIGKILL or machine failure can leave an admitted record without
a completion record; that absence must not be treated as confirmed cleanup.
See [Shepherd's platform guarantees](https://github.com/nessalabs/shepherd#platform-guarantees-current).
These records are private local evidence, not tamper-proof against programs with
the same OS user's privileges.

## Layout and checks

- `src/mcp.rs`: generic MCP transport and tool routing.
- `src/shell/domain/`: validated command, invocation identity, and execution causes.
- `src/shell/application/`: command orchestration, result evidence, and injected runner/audit ports.
- `src/shell/infrastructure/`: Shepherd and private audit adapters.
- `tests/stdio.rs`: real transport, commands, cancellation, EOF and output limits.
- `tests/shell/domain/`: command value and cause tests.
- `tests/shell/application/`: application substitution and audit-gate tests.
- `tests/shell/infrastructure/`: Shepherd cleanup and process identity tests.

```sh
cargo test -p nessa-mcp
cargo clippy -p nessa-mcp --all-targets -- -D warnings
```
