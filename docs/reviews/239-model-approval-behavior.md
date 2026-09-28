# Additional model approval behavior

Follow-up to [issue #239](https://github.com/nessalabs/nessa-agent/issues/239)
and [ADR 231](../adr/todo/231-model-and-approval-per-conversation.md).

The opt-in `live_presets` SDK contract probe uses the production Claude/Codex
bindings. The elevation matrix opens in Ask, applies a candidate preset through
the session RPC,
performs a disposable action matrix, then returns to Ask and repeats it with
fresh output paths. It opens a fresh session for the other candidate preset.
The initial elevation probes do not bypass construction's catalog check; the
separate downgrade probe uses verified candidates at construction.

| Phase | Evidence required |
| --- | --- |
| Select Auto / Full | Production binding verifies the exact effective model and mode; a fallback is a failed selection. |
| Read | Native-tool result for workspace and outside-workspace canaries. |
| Edit | Exact bytes in separate workspace and outside-workspace output files. |
| MCP | Independent append-only marker written by the fixture MCP server. |
| Shell | Nessa MCP shell result and private process-audit records for a harmless printf. |
| Network | Native WebFetch (Claude) or native shell curl (Codex) against example.com; distinguish policy refusal from transport failure. |
| Denied tools | Claude schema lookup for representative denied native tools; no prohibited action is requested. Codex has no analogous Nessa native-tool deny list. |
| Downgrade | Exact Ask selection and repeated actions with action permission requests rejected, recording both requested decisions and side effects. |

All action approval requests are rejected. ToolSearch schema discovery alone is
allowed. Native automatic review can itself allow or deny an action; an agent's
statement of intent or a successful configuration reply is not an observed effect.
A test exit alone does not establish the matrix: review the recorded observations
and independent file/MCP/shell witnesses before changing availability.

## Reproducing

Use existing local provider authentication and the pinned adapters. Run from the
repository root, with a fresh evidence directory (the probe refuses to overwrite
an existing model report):

```sh
NESSA_LIVE_MODEL=claude-opus-5 \
NESSA_LIVE_CLAUDE_ACP=/absolute/path/to/claude-agent-acp \
NESSA_LIVE_MCP=/absolute/path/to/nessa-mcp \
NESSA_LIVE_EVIDENCE=/absolute/path/to/new-evidence \
cargo test -p nessa-sdk --lib candidate_presets_record_real_tool_behavior_and_downgrade -- --ignored --nocapture
```

For Codex, supply `NESSA_LIVE_CODEX_ACP` and its exact catalog model ID instead.
Repeat with the test filter `codex_presets_downgrade_each_approval_boundary` and
a fresh evidence directory to isolate the Ask approvals. This probe is for
Codex only. Supply the same adapter and MCP environment variables for both.
The probe uses temporary workspace and outside-workspace directories and removes
them at the end. It preserves JSONL observations and Nessa shell audit records in
the evidence directory. Reports may contain local paths and should be normalized
and inspected before publication. Credentials are never deliberately recorded.

## Platform availability

The additional choices in this report are published only on macOS. Linux,
Windows and other targets retain Ask for those models until their behavioral
matrix is verified. Existing Sonnet/Astra choices are unchanged. Provider
bindings own this target-dependent rule; construction and catalog publication
consume the same choices.

| Target/model | Published choices and construction |
| --- | --- |
| macOS, newly verified Auto/Full model | Ask, Auto, Full |
| macOS, Haiku | Ask, Full; reject Auto |
| Other target, newly verified model (including Haiku) | Ask; reject Auto and Full before process launch |
| Unknown model | Ask; reject Auto and Full |

## Results

Existing Sonnet and Astra presets retain the original ADR 231 evidence; this
follow-up verifies only the additional model choices.

The matrix was run on macOS arm64 with Claude ACP 0.76.0 and Codex ACP
1.12.0, existing local authentication, and the installed production Nessa MCP.
[Normalized observations](evidence/239-model-approval-macos.json) retain tool
identities, permission inputs, exact output files, MCP marker logs, and shell
audit records; model message chunks are omitted. The MCP executable's SHA-256
is recorded because the installed binary is used rather than rebuilt by this
probe.

| Newly verified model | Auto | Full | Return to Ask |
| --- | --- | --- | --- |
| Claude Opus 5 | Verified | Verified | Outside read, both writes, MCP, shell and WebFetch reach host approval; denied actions have no observed effects. |
| Claude Fable 5.1 | Verified | Verified | Same boundaries as Opus. |
| Claude Haiku 4.5 | Unavailable | Verified | Same boundaries after Full. |
| Codex Sol / Terra / Luna | Verified | Verified | Outside file, MCP, Nessa shell and network escalation reach host approval; denials produce no corresponding effects. |

Claude elevated runs read both canaries, write the exact expected file bytes,
append the MCP marker, run Nessa shell with exit 0 and `SHELL_239` output, and
fetch Example Domain. Lookup of the sampled denied tools (`Bash`,
`EnterPlanMode`, `ExitPlanMode`, `CronCreate`, `Monitor`) returns no matching
schemas. This is a representative lookup check; ADR 231 owns the broader deny
list investigation. Haiku's Auto selection fails with a typed protocol failure
and cleanup-required state in the production binding. The earlier direct
harness observed its `acceptEdits` fallback. Neither result qualifies as Auto,
so that choice stays unavailable.

Codex Ask permits workspace edits and ordinary native shell actions. Its outside
file edit requires approval; rejecting it cancels the turn before later MCP and
network steps. The separate `codex_presets_downgrade_each_approval_boundary`
probe therefore starts a fresh session in each elevated preset, switches to Ask,
and requests one MCP, Nessa shell or network action. This distinguishes actual
approval from an action the cancelled turn never attempted. No native-tool deny
list is configured for Codex.

The final Codex rerun exposed two probe limitations, retained rather than counted
as successes: Luna mistyped an outside scratch path, and Terra substituted its
native shell for the requested Nessa MCP shell on two downgraded Auto turns.
The Luna repeat supplies both native read canaries
and the intended file bytes. Naming the MCP server and tool explicitly in the
Terra probe supplies the missing Auto-to-Ask shell approval. The discarded
attempts and the repeats have distinct `runId` values in the evidence; they are
not silently combined into a successful run.

The completed Codex verification runs produce both read canaries, exact files, MCP
markers and two independently audited Nessa shell completions per model/run.
Auto's native curl first encounters the sandbox's DNS refusal, then native
Guardian review approves escalation and the tool returns HTTP 200. Full returns
HTTP 200 directly. On downgrade, file/network denials cancel their turns, while
MCP/shell denials return a tool refusal and a completed turn. No downgrade run
writes the MCP marker or admits a Nessa shell command.

## Regression scope and limits

The controlled open/resume matrices cover the binding-owned model choices and
the server catalog derives its choices from those bindings. Unknown models and
Haiku Auto remain rejected. Mapper tests cover current file-edit locations,
sparse MCP inputs, missing/malformed evidence, repeated and conflicting inputs,
aggregate retained-byte bounds and execution reset. The MCP process contract
checks Allow and Deny through the host and preserves original input, request and
execution identity, attribution, and Selected/Written audit stages.

These live runs establish behavior for the pinned adapters and benign test
actions on this macOS host. They do not claim that a native automatic classifier
will approve every future action, or establish Windows/Linux behavior. Controlled
regressions run without credentials in the existing CI matrix. No new public API,
persistence format, approval policy, or frontend capability table is introduced.

## Captured Codex approval shapes

The pinned adapter supplies file-edit locations on the approval's `toolCall`.
For MCP, it supplies arguments on an earlier `tool_call` notification and later
requests approval using only that same tool ID and kind. Command approvals carry
`rawInput` directly. The obsolete `_meta.codex` fixture did not represent these
live shapes.

The adapter retains the original input by tool-call ID until execution reset,
with a separate aggregate encoded-input budget of 1 MiB. The regression
`sparse_mcp_review_uses_only_the_original_input_for_its_tool_id` checks that
foreign tool identities cannot supply its arguments. Input retention has these
cases:

| Incoming observation/request | Retention and review |
| --- | --- |
| First object input for a tool ID | Retain exact JSON within the adapter input budget. |
| Sparse update with input absent/null | Keep original input. |
| Repeated equal object | Idempotent; no additional charge. |
| Different object for the same tool ID | Reject before replacing retained input. |
| Malformed input or aggregate budget exceeded | Reject before retention. |
| Approval with its original input | Review that input, checking agreement with retained input. |
| Approval without input, known tool ID | Review the retained input for that identity. |
| Edit approval without input or retained input | Review its validated nonempty locations and request metadata. |
| Other approval without evidence | Refuse as unreadable. |
| Next execution | Clear tool identities, retained input and its accounting together. |

## Local validation

Seven temporary revert probes each failed the intended regression, then the
original source was restored: Claude/Codex availability, file-edit request
mapping, sparse MCP input recovery, input agreement, aggregate input bounds,
and reset accounting. The restored mapper suite passes, including rejection
before retention and valid re-admission after reset.

On macOS arm64, the combined CI package selection passed:
`cargo test -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-sdk`
and the same packages' `cargo clippy --all-targets -- -D warnings`. This includes
496 SDK and 1170 server library tests, plus integration/doc tests. The tests ran
with their default concurrency; the final Terra live probe overlapped part of
the build/test run. Rust formatting, architecture checks, SDK documentation
checks and `git diff --check` also passed. Live provider tests remain opt-in;
no CI jobs are added.

## PR #240 CI follow-up

The first Linux/Windows CI run found two existing Node archive identity test
failures. Windows inode values exceeded JavaScript's safe integer precision;
identity-carrying filesystem stats now require BigInt. The Linux substitution
fixture had deleted the sole link before creating a replacement, allowing inode
recycling; it must retain the original file to establish a distinct replacement.
The cooperative-writer and verified-returned-bytes contract stays unchanged.

| Case | Required result |
| --- | --- |
| Same hard-linked file with identity above 2^53 | Accept its verified publication |
| Distinct files whose IDs round to the same Number | Refuse the claimed publication, preserving the foreign destination |
| Source renamed away then replaced by a symlink | Report publication and cleanup failures, preserve the retained source and symlink target |
