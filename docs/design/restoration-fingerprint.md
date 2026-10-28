# Restoration fingerprint and a context-only mismatch

A saved conversation is resumed on the native session it already has only when
the provider name, the model, and the restoration fingerprint all match, and
the provider can restore that session here. Anything else that is only the
fingerprint opens a new native session and keeps the history. A different
provider name or model is still refused.

Issue [#1071](https://github.com/nessalabs/nessa-agent/issues/1071). The
fingerprint's inputs are listed once, on `fingerprint` in
`crates/nessa-sdk/src/infrastructure/acp/sessions/identity.rs`. This note is
the state table for what `SessionManager::prepare` and `attach` do with that
fingerprint (gate 15).

## What selects a saved native session

The restoration fingerprint's inputs are listed once, on `fingerprint` in
`crates/nessa-sdk/src/infrastructure/acp/sessions/identity.rs`. This is that
list.

In the restoration identity:

- the provider name and the model, beside the hash on `ProviderIdentity`;
- every entry of the identity environment (`AcpConfig::environment`): vendor
  account directories (`HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, the
  OpenCode `XDG_*` roots) and endpoint or profile selectors
  (`ANTHROPIC_BASE_URL`, `AWS_PROFILE`, and the same kind of variable);
- every argument, in order. A subcommand, a flag, or a profile or model
  selector, including an absolute path such as `--config /profiles/staging.toml`.
  Nothing infers a staged runtime path from the argument's shape;
- the workspace, whether tools are enabled, the token limits, the permission
  decisions, and the system prompt.

Deliberately not in it. These are the warm-up launch fingerprint:

- bundled runtime paths with explicit staged-runtime provenance: the
  executable path, and `AcpConfig::staged_runtime_arguments`, which
  composition sets from the bundled runtime location Nessa itself placed
  (the adapter entry beside that executable). A release stages that tree
  under a new directory, and relocating it renames the files. A path in
  `arguments` is not one of these. Which implementation a conversation
  belongs to is the provider name, the model, the arguments, and the
  identity environment;
- `PATH`, `TMPDIR`, `USER`, `LOGNAME`, `CODEX_PATH`, and
  `CLAUDE_CODE_EXECUTABLE`, which composition keeps in
  `AcpConfig::runtime_environment`. On macOS, `PATH` is `NESSA_AGENT_PATH`
  from the login shell, rewritten into the launchd job whenever an update
  re-registers it, and `TMPDIR` differs between launchd and an app-launched
  child;
- credentials and MCP servers.

Warm-up keeps a launch fingerprint of its own: the executable, the
staged-runtime arguments, the arguments, and the runtime environment. It is
recorded beside the
restoration fingerprint, so a moved runtime re-runs warm-up and does not
change the restoration identity. A saved conversation whose old fingerprint
included the executable or the whole environment does not match. It takes
the context-only row below and opens a new native session. Nessa is alpha
and has no users, so the old form is not accepted.

## States

`prepare` then `attach`, for a conversation that already has a snapshot.
"Restores" means `restores_saved_context` is true for the saved native
session id.

| Row | Name | Model | Fingerprint | Restores | What happens |
| --- | --- | --- | --- | --- | --- |
| S1 | differs | any | any | any | `IdentityMismatch`. Nothing is launched. The snapshot is unchanged. |
| S2 | matches | differs | any | any | `IdentityMismatch`. Nothing is launched. The snapshot is unchanged. |
| S3 | matches | matches | matches | yes | The saved native session is resumed. |
| S4 | matches | matches | matches | no | A new native session is opened. `ProviderContext` records it. The fingerprint is unchanged. |
| S5 | matches | matches | differs | no | `ProviderIdentityChanged` is saved, then a new native session is opened and recorded. This is the existing path for a context that lives on another worker. |
| S6 | matches | matches | differs | yes | The saved native session is not resumed. A new one is opened. `ProviderIdentityChanged` and the new `ProviderContext` are one save, after the new session exists. |

S6 is the row that used to refuse with `storage-identity` and strand the
conversation. A moved staged runtime, and `PATH` or `TMPDIR` drift, no
longer reach S6: they are not in the restoration fingerprint, so the row
is S3 and the saved native session is resumed. An endpoint, profile
selector, or argument changing — including an absolute `--config` value —
with the same provider name and model, is S6: the saved native session id
belongs to the previous context.

## Invariants

1. A saved native session is resumed only on S3. `attach` passes the saved
   id into `open` only when the durable provider identity still equals the
   current one and `restores_saved_context` is true.
2. S1 and S2 launch nothing and write nothing. A test that changes the
   provider name or the model and then prepares fails with
   `IdentityMismatch`, and the snapshot is the one from before the call.
3. S6 does not write the new fingerprint before the new native session id
   exists. A crash between `prepare` and that save leaves the durable
   identity and context as they were, so the next open takes S6 again
   instead of S3 under the new fingerprint with the old session.
4. S5 still writes `ProviderIdentityChanged` in `prepare`. A crash there is
   safe because `restores_saved_context` stays false, so the next open does
   not resume the other worker's session.
5. The composer shows the conversation record's model (`selection`), including
   when no provider is attached. `wire_view` puts that model on the view.
   The desktop reads it. It does not substitute the catalogue default.
6. A model change that the window does not send, or that the gateway refuses
   as `conversation_configuration_changed`, says the chat cannot be reopened.
   An uncertain model change that is not that refusal still says the change
   is not confirmed.
7. The executable path, the staged-runtime arguments composition recorded,
   and the runtime environment are the warm-up's launch fingerprint, not
   the restoration fingerprint. Moving them leaves S3. Warm-up's key
   changes, so the next boot prepares that runtime again. S1 and S2 are
   unchanged.
8. An endpoint or profile variable in the identity environment is part of
   the restoration fingerprint. Changing it is S6, not a resume of the
   saved native session id, and it does not change the launch fingerprint.
9. Every argument is part of the restoration fingerprint, including an
   absolute path. Changing `--profile staging` or `--config
   /profiles/staging.toml` is S6. It also changes the launch fingerprint.
   Only a path in `staged_runtime_arguments` is left out.

## Alternatives

- **Keep hashing the executable, the arguments, `PATH`, and `TMPDIR`, and
  keep refusing S6.** This is the bug. Every release stages the runtime
  under a new directory, and an update also re-registers the launchd job,
  so the shell `PATH` is snapshotted again. Either change refuses every
  saved conversation of that agent for good. Retrying cannot succeed.
- **Accept the previous fingerprint once** (the issue's step 1). Alpha has
  no users and no migration. S6 already opens a new native session when the
  stored fingerprint was computed from the wider list, so the conversation
  stays usable without a second format.
- **Refuse S6 only when the provider would resume, and open fresh only on
  S5.** That keeps a worker from replacing another worker's native session.
  It also leaves every conversation saved before this change refused, which
  is the strand the issue is fixing. Losing that native session once is the
  cost of not refusing the conversation.
- **Clear the saved context to `Absent` when the fingerprint differs.**
  Provider evidence forbids `Absent` once a turn has run, so the history
  would no longer validate.
- **Show the catalogue default until an agent attaches.** That is what the
  desktop does when the view has no model. A Codex conversation then shows
  Claude Opus 5, and a model pick fails in the window before the gateway
  sees it. The product view's `model` is required, so a view always carries
  the conversation's selected model.
- **Treat every non-directory environment key as plumbing.** An endpoint or
  profile selector left in the identity environment is forwarded to the
  child. Leaving it out of the fingerprint resumes a native session that
  belongs to the previous backend. Runtime paths stay in the runtime
  environment, which the restoration fingerprint does not read.
- **Treat every argument as plumbing, including `--profile staging`.** The
  binding passes the argument to the child. Leaving it out of the
  fingerprint resumes a native session that belongs to the previous profile.
  Only a path composition recorded as staged runtime stays out.
- **Treat every absolute argument as a staged runtime path.** `--config
  /profiles/staging.toml` is absolute and selects a profile. Inferring
  provenance from `Path::is_absolute` resumes the previous profile's native
  session. The staged runtime is the location Nessa recorded, not a shape
  an argument happens to have.
- **Hash the executable path or its file name.** A release moves the staged
  directory, and relocating that tree renames the binary. Either hash
  changes the restoration identity on a move that should resume.

## Consequences

A change to a hashed input no longer refuses the conversation. The history
continues and the native session starts over. A moved staged runtime is not
a hashed input: the saved native session is resumed, and warm-up records
the new launch fingerprint separately. `docs/design/mcp-connections.md`
("What still strands a saved conversation") says the same thing: only a
different provider name or model refuses; a fingerprint change opens a new
native session.

A provider that still carries a replaced fingerprint and could resume the
saved id (the old pool refusal) now opens a new native session instead of
refusing. It does not resume the other worker's session.
