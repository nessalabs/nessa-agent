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
Diagnostics, including the package install, go to stderr. `run-codex.sh` and
`run-opencode.sh` accept `--only <row>`. `--only` without `--out` prints
`--only needs --out` and exits 2. `--only` whose `--out` resolves to the
committed evidence directory prints `--only cannot write to the committed
evidence directory` and exits 2, before anything there is removed. An `--only`
value that is not one of that runner's rows prints
`unknown row: <name>` and exits 2. A full run, with no `--only`, is unchanged.

`run-claude.sh` reads `claude --version` before it runs and exits unless that
version is 2.1.257, the surveyed CLI. The compared token is the first word of
that line, `2.1.257` in `2.1.257 (Claude Code)`. `--allow-claude-version <v>`
allows `<v>` instead and records `allowed_claude_version` in `summary.json`.
Without the flag the summary has no such field. The script uses
`env -u ANTHROPIC_API_KEY` and `--model claude-haiku-4-5-20251001`. Every
`claude` process, including resume and `/compact`, is started with
`(cd "$project" && ...)`.
`--dangerously-skip-permissions` is passed only on those invocations, and
`events-permission` omits it. `CLAUDE_CONFIG_DIR` is the real login directory.
Credential files are not copied. The script itself does not write that
directory, but the Claude CLI writes session transcripts there (see below).
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
or the home directory. After it writes the summary, it also exits non-zero
when a row's exit code is not 0, and names that row on stderr. Codex
`stop-decision-block` with suppression off is expected to exit 124.
`events-resume` and `events-compact` record a non-zero exit code when the
first run has no session id, and that follow-up run is not attempted.
`pretool-allow-updated` asks for a Bash `touch` of
`$SENTINEL/hook-sentinel-original` and passes only when the sentinel directory
contains exactly `hook-sentinel-rewritten`.

The reference checks require the six events on the plain and tool rows, the
unsuppressed `events-plain` row to exit 0 with both `project` and `user` hook
sources, an empty `suppress-settings` log that exits 0 with `terminal_reason`
`completed`, a PreToolUse deny whose `permission_denials` has at least one
entry and whose log has no PostToolUse, the rewritten sentinel alone, and a
write from `control-user` that exits 0. Whether the model quoted the deny
reason is `deny_reason_quoted`. That fact is not part of the pass.

`--managed` runs two extra rows, `suppress-managed` (the managed file only)
and `suppress-managed-plus-flag` (that file plus
`--settings '{"disableAllHooks":true}'`). Managed hooks still fire with that
flag, and the two rows match. The reference check does not require those logs
to be empty. Before evidence is published, the script restores or removes
`/etc/claude-code/managed-settings.json`. If that restore fails, nothing is
published, the backup and work directory stay, and the script exits non-zero.
The exit trap does that restore when the script stops earlier. A failed
restore or removal leaves the work directory in place and exits non-zero.
After a successful restore, the trap deletes the work directory. Rows and
the summary are published as one directory snapshot, so a failed copy leaves
the previous pair in place.

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

Before `run-codex.sh` and `run-opencode.sh` record a row, they rewrite
`$WORKDIR` to `<workdir>` and `$HOME` to `<home>` in that row's stdout, stderr,
and request files. `run-claude.sh` does not do that rewrite.

The committed logs are `evidence/codex`, `evidence/opencode`, and
`evidence/claude`. The Claude log includes `live-acp-test.log`.
