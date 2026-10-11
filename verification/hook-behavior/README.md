# Hook behavior

Scripts that drive a real harness and record which hook events fire and which
outputs that harness accepts. They are not a CI gate. Claude needs the login on
the machine that holds it. Codex and OpenCode install their pinned packages
into a scratch prefix and talk to `scripted_openai.py`.

```sh
verification/hook-behavior/run-claude.sh
verification/hook-behavior/run-claude.sh --managed
verification/hook-behavior/run-codex.sh
verification/hook-behavior/run-opencode.sh
```

`--out` sets the directory for `rows.jsonl` and `summary.json`. The default is
`verification/hook-behavior/evidence/<harness>`. stdout is the summary JSON.
Diagnostics, including the package install, go to stderr.

`run-claude.sh` uses `env -u ANTHROPIC_API_KEY` and
`--model claude-haiku-4-5-20251001`. Every `claude` process, including resume
and `/compact`, is started with `(cd "$project" && ...)`.
`--dangerously-skip-permissions` is passed only on those invocations, and
`events-permission` omits it. `CLAUDE_CONFIG_DIR` is the real login directory.
Credential files are not copied, and the script does not write that directory.
The user hook is `--settings` pointing at a file inside the project. Project
hooks are `$project/.claude/settings.json`. Each command is
`hook_cmd.py --source project|user|managed`, so the log names the source.
`control-user` is the positive control: the user hook only, and the reference
check fails when `hook_source` `user` is absent. Context and system rows use
this prompt:

```
If any extra context or system message contains a token starting NONCE_ or SYSMSG_, reply with exactly those tokens; otherwise reply NONE.
```

The Claude CLI writes session transcripts into the login directory. For Claude,
`context_nonce_in_result` and `system_nonce_in_result` are what the model
echoed. `context_nonce_in_transcript` and `system_nonce_in_transcript` are
presence in the session transcript. Transcript presence is not a model request.
Those two rows store only those booleans. Every other Claude row stores
`result` capped at 200 characters, plus `terminal_reason` and
`permission_denials`. Notes say `<login-dir>` and `<workdir>` instead of those
paths. `finalize` exits non-zero when any stored text contains an email address
or the home directory.
`pretool-allow-updated` asks for a Bash `touch` of
`$SENTINEL/hook-sentinel-original` and passes only when the sentinel directory
contains exactly `hook-sentinel-rewritten`.

The reference checks require the six events on the plain and tool rows,
an empty hook log for `suppress-settings`, a PreToolUse deny whose
`permission_denials` has at least one entry and whose log has no PostToolUse,
the rewritten sentinel alone, and a write from `control-user`. Whether the
model quoted the deny reason is `deny_reason_quoted`. That fact is not part of
the pass.

`--managed` runs two extra rows, `suppress-managed` (the managed file only)
and `suppress-managed-plus-flag` (that file plus
`--settings '{"disableAllHooks":true}'`). Managed hooks still fire with that
flag, and the two rows match. The reference check does not require those logs
to be empty. The exit trap restores or removes
`/etc/claude-code/managed-settings.json` and then deletes the work directory.

Codex and OpenCode results are labeled `real harness, scripted model`. Codex
`config.toml` leaves hooks enabled and does not set `notify`. Suppression on
passes `-c features.hooks=false -c 'notify=[]'` on `exec` and on `resume`. That
is the hooks portion of Nessa's thread/start override, not the full
`CODEX_CONFIG` document. User and project command hooks are tagged
`--source user` and `--source project`.

OpenCode plants a project plugin and a user plugin under the private
`XDG_CONFIG_HOME/opencode/plugin`. Each copy records its own `hook_source`.
The `on` variant sets `OPENCODE_PURE=1` and `OPENCODE_DISABLE_PROJECT_CONFIG=1`.
`suppress-pure` sets only `OPENCODE_PURE=1`. The three alternate config
variables are unset on every row.

The committed logs are `evidence/codex` and `evidence/opencode`. Claude's log
is not committed until the machine that holds the login runs `run-claude.sh`.
