# Local Opencode ACP binding

This provider guide covers the local Opencode harness. Start with the
[agent execution guide](agent_execution/README.md) for domain ownership,
lifecycle, permissions, prompts, and shared transport contracts, and read the
[Claude guide](claude-acp.md) alongside it: everything the three bindings share
— process supervision, restoration identity, queueing and steering, the hook
contract, the credential and context environment split — is stated there and is
not repeated here. What follows is what is Opencode's own.

Unlike the other two, this profile was written from a recording rather than from
documentation. The installed 1.18.31 binary was probed over ACP, and which
frames are observed and which are written from the ACP specification is stated
in the fixture, in the module, and on the functions that read them.

## Composition and application boundary

Identical to the other two bindings in shape. The host supplies a selected
`ModelMetadata`, explicit `TokenLimits`, an
`infrastructure::acp::sessions::AcpConfig`, and an `Arc<dyn ExecutionAudit>` to
`OpencodeAcpProvider::new`. The factory is immutable, and
`Agent::prepare(provider, manager, audit)` loads durable evidence; `authorize_attachment` and `start_attachment` open or resume the context.

```text
host -> Agent(provider, SessionManager)
          |-> OpencodeAcpProvider -> ProviderSession -> ACP worker
          |-> SessionManager -> SessionStorageLease
          |-> hooks + optional UI subscribers
```

What differs is behind `AcpProfile`:

- **Session configuration is an ordered list**, the same shape Codex needs, and
  the shared verification for it now lives in
  `infrastructure/acp/sessions/configuration.rs` rather than under `codex_acp/`
  — the two agents name the same two options and were about to verify them
  twice. Opencode's steps are `model` then `mode`, and `verify_update` refuses a
  mode change away from `plan` once the configuration has landed.
- **There is no system prompt, and no way to pass one.** Opencode accepts
  `instructions`, `systemPrompt` and `_meta.systemPrompt` on `session/new`
  without complaint, and accepts a nonsense field just as readily, so acceptance
  carries no information about what it read. The binding offers no
  `with_system_prompt` at all, which is what stops composition passing one by
  accident. Opencode therefore runs under its own instructions.
- **A permission request is named by its ACP kind, never by its title.** A title
  is display text Opencode composes for a person, and on a permission request it
  is built from the model's own arguments for some categories, so it cannot be
  the identity an approval is recorded under. A request whose kind is `other` —
  the kind Opencode maps everything it has no case for to, including this
  server's own MCP tools — carries no reviewable identity and is refused. The
  reasoning, and what would rescue the `other` case, is on
  `opencode_acp/tools/wire.rs`.

## Credentials and private process roots

The SDK binding does not decide which OpenCode model or credential policy a
product supports. A caller supplies the selected model separately and may place
an admitted credential in `AcpConfig::credential_environment`. The binding does
not discover an OpenCode account, read `auth.json`, or promise that a free or
metered catalogue entry is eligible for that caller.

Nessa's packaged composition requires a saved stage-scoped API key and starts
on the metered `opencode/minimax-m3` Zen model. Standalone composition preserves
the explicit runtime policy and captures `OPENCODE_API_KEY` when composition
starts. Those are composition decisions rather than general restrictions on
the SDK or on models upstream may offer.

Before launch the binding removes `HOME`, all four XDG roots, and alternate
OpenCode config inputs from both environment maps. It gives the process fresh
private HOME, config, data, cache, and state roots, disables project config and
external plugins, and therefore exposes none of the caller's plugins, provider
configuration, or account data. The admitted credential environment is the
only credential path into the process.

## What bounds a session

Not the mode's name. In 1.18.31 `plan` denies the `edit` tool outside its own
plans directory and nothing else; `bash`, `webfetch`, `websearch`, `task` and
every MCP tool stay at the global default of `allow`. Nessa hands each session
an MCP server whose one tool is a shell, so a session bounded by the mode alone
is handed a shell.

The policy is pinned at launch instead, where ACP cannot reach it:
`OPENCODE_PERMISSION` carries a deny-first policy allowing only reading and
searching, `OPENCODE_DISABLE_PROJECT_CONFIG` takes the opened checkout out of
the config search, and `OPENCODE_PURE` empties the external plugin list so no
plugin tool is registered and no plugin hook can rewrite the arguments of a tool
the policy did allow. Two more close things a launch would otherwise do on its
own rather than anything a tool can reach: `OPENCODE_DISABLE_MODELS_FETCH` stops
the models.dev refresh that runs at startup and hourly after, which a session
whose model Nessa already chose has no use for, and
`OPENCODE_DISABLE_AUTOUPDATE` keeps the pinned copy pinned.

What that policy does **not** cover is written out on the constant in
`opencode_acp/sessions/binding.rs`, because a bound believed to be wider than it
is is worse than a narrow one. Read it there rather than here: it is the kind of
statement that has to sit beside the value it qualifies.

## Install and run

Opencode is not bundled. In packaged Nessa, composition resolves only the
managed runtime at Nessa's exact current pin and retains its executable-use
authority through provider lifetime. Missing, stale, unsupported, or unreadable
managed state is refused rather than replaced by a command path from config.
Standalone composition instead uses its explicit trusted command and arguments;
the SDK itself accepts the caller-owned executable snapshot in `AcpConfig`.

The release is pinned and the launch sets `OPENCODE_DISABLE_AUTOUPDATE`, so a
copy only moves when Nessa ships. Only Opencode's own TUI reaches the upgrade
path and `acp` never does, so the variable makes that true of the binary rather
than of the entry point — somebody who also runs the TUI would otherwise
replace the version this profile's `initialize` check is written against, and
see `requires Opencode 1.18.31` with nothing saying why.

## Verification

Contract tests in `tests/infrastructure/acp/contracts/opencode.rs` run against
`fixtures/opencode_acp_test_handler.py`, which serves the protocol shapes the
real binary was recorded answering: `agentInfo { name: "OpenCode", version }`,
no `_meta/steering`, and `session/new` with the `model` and `mode` config
options. The fixture also verifies the private environment and pinned launch
settings, so widening that boundary fails deterministically.

Not claimed: a successful paid model turn. The fixture proves protocol shape
and launch isolation; it does not prove that a saved key is accepted by the
live service or that the selected model will answer.
