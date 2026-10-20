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

Hashed: the executable, the ordered arguments, the workspace, the token
limits, the permission decisions, the system prompt, and the environment keys
that name where the vendor keeps its account:

- `HOME`
- `CLAUDE_CONFIG_DIR`
- `CODEX_HOME`
- `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`
  (OpenCode's config roots; the same fact as the vendor directory)

Not hashed, the same way credentials and MCP servers already are not:
`PATH`, `TMPDIR`, `USER`, `LOGNAME`, `CODEX_PATH`, `CLAUDE_CODE_EXECUTABLE`,
and any other environment key. Those are process plumbing. On macOS, `PATH`
is `NESSA_AGENT_PATH` from the login shell, rewritten into the launchd job
whenever an update re-registers it, and `TMPDIR` differs between launchd and
an app-launched child. Neither chooses which native session to resume.

There is no reader for a fingerprint computed by the previous list. A saved
conversation whose fingerprint was computed from the whole environment does
not match. It takes the context-only row below and opens a new native
session. Nessa is alpha and has no users, so the old form is not accepted.

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
conversation. `PATH` or `TMPDIR` drift no longer reaches S6: those keys are
not in the fingerprint, so the row is S3.

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

## Alternatives

- **Keep hashing `PATH` and `TMPDIR`, and keep refusing S6.** This is the
  bug. An app update re-registers the launchd job, the shell `PATH` is
  snapshotted again, and every saved conversation of that agent is refused
  for good. Retrying cannot succeed.
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
  sees it.

## Consequences

A change to a hashed input, including the staged adapter path moving on an
update, no longer refuses the conversation. The history continues and the
native session starts over. `docs/design/mcp-connections.md` ("What still
strands a saved conversation") is updated to say that: only a different
provider name or model refuses; a fingerprint change opens a new native
session.

A provider that still carries a replaced fingerprint and could resume the
saved id (the old pool refusal) now opens a new native session instead of
refusing. It does not resume the other worker's session.
