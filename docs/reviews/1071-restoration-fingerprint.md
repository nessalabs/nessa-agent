# Review brief: restoration fingerprint (#1071)

Pre-implementation review of
[the design note](../design/restoration-fingerprint.md) against
[the local code review gate](../../CODING_STANDARDS.md#local-code-review-gate).
No product code had been edited. The tree was `main` at the review, plus this
note and the design note.

## Brief

- **Checkout:** `nessalabs/nessa-agent`, branch `main`, plus the two uncommitted
  documents named above. No product diff.
- **Intended behavior:** `PATH`, `TMPDIR`, `USER`, `LOGNAME`, `CODEX_PATH`, and
  `CLAUDE_CODE_EXECUTABLE` do not change the restoration fingerprint. A
  fingerprint mismatch with the same provider name and model opens a new
  native session and records `ProviderIdentityChanged`, in one save with the
  new context when the provider would otherwise resume the old one. A
  different name or model is still `IdentityMismatch`. The desktop shows the
  conversation record's model when no runtime is attached, and says the chat
  cannot be reopened when a model change is not sent or is refused
  `conversation_configuration_changed`. A stress step restarts the gateway
  with a changed agent `PATH` and `TMPDIR` and expects the next send to an
  existing conversation to complete.
- **Owned scope:** `fingerprint` inputs; `SessionManager::prepare` and
  `attach` for rows S1–S6; the product view's selected model; the desktop
  composer model and the model-change sentence; the conversation-stress
  step. Not in scope: launchd `PATH` registration, provider name or model
  refusal, migrations, issues #965, #981, #1038, #1039, #1030, #1034, #345,
  #337, #746.
- **Known findings:** none yet. The cloud reproduction in the issue is the
  behavior this review is checking the design against. It had not been
  re-run in this checkout at the time of the review.

## What was inspected

- `crates/nessa-sdk/src/infrastructure/acp/sessions/identity.rs` — the
  fingerprint hashes the whole `AcpConfig::environment`.
- `crates/nessa-sdk/src/application/agent_execution/sessions/manager.rs`
  `prepare` and `attach` — a fingerprint mismatch that would resume the
  saved context returns `IdentityMismatch`; the not-restorable path records
  `ProviderIdentityChanged` and opens fresh.
- `ProviderContext::validate_evidence` — `Absent` is illegal once a turn has
  provider evidence, so S6 cannot clear the context.
- `crates/nessa-server/src/composition/agent.rs` `process_environment` and
  `provider_for_managed_adapter` — `PATH` comes from `NESSA_AGENT_PATH`,
  and `CODEX_PATH` / `CLAUDE_CODE_EXECUTABLE` are merged into the same map.
- `crates/nessa-server/src/product/conversation.rs` `wire_view` — the
  selected model is copied onto `runtime` only when a runtime exists.
  `read_unopened` sets `selection` and not `runtime`.
- `src/desktop/workspace/adapters/gateway/gateway-views.ts` `runningModel`
  and `gateway-source.ts` `setModel` — no runtime means the catalogue
  default, and `setModel` throws an uncertain `unavailable` before any RPC.
- `packages/nessa-client` `rejectedBeforeDispatch` —
  `conversation_configuration_changed` is not classified as rejected before
  dispatch, so a gateway refusal of that code is uncertain. The sentence
  change is in the composer, not in that classification.
- Gates 13, 15, and 16, and the agreement check: one fingerprint function;
  the state table; name and model stay a hard refusal; the new fingerprint
  and the new context are one save on S6 so a crash cannot resume the old
  session under the new fingerprint.

## Agreement

Authoritative facts: provider name, model id, and context fingerprint live
on `ProviderIdentity`; the native session id lives on `ProviderContext`;
the conversation's selected model lives on the metadata record and on
`ConversationSelectionView`. `prepare` is the owner of S1, S2, and whether
S5 writes `ProviderIdentityChanged` before launch. `attach` is the owner of
which session id is passed to `open`, and of the S6 save. `wire_view` is
the owner of the model id on the product view. The desktop reads that
field; it does not derive a model of its own when the field is present.

Checked combination: same name and model, different fingerprint, provider
would resume (S6). Writing the fingerprint first and then crashing would
make the next open look like S3 and resume a session that belongs to the
old fingerprint. The design forbids that write until the new session id is
in the same unit. The opposite combination, S3, still resumes.

## Result

No finding at any priority. Implementation may follow the table. XDG roots
stay in the allowlist as OpenCode's vendor directories, the same rule as
`CODEX_HOME` and `CLAUDE_CONFIG_DIR`, not a second list.

Unverified until the code exists: the recipe in the issue (restart with
`NESSA_AGENT_PATH`, then with `TMPDIR`), the S6 crash retry, and the
stress step.
