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

`run-claude.sh` uses `env -u ANTHROPIC_API_KEY`, one temporary project per row,
and `--model claude-haiku-4-5-20251001`. `--managed` plants
`/etc/claude-code/managed-settings.json` with sudo and restores or removes it.
The summary's `reference_checks` are the Claude 2.1.257 Haiku observation:
SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, and SessionEnd
fire; `disableAllHooks` via `--settings` produces no events; a PreToolUse deny
stops the Read and the model quotes the reason.

Codex and OpenCode results are labeled `real harness, scripted model`. Each row
runs with the planted native hook and again with the suppression request the
binding sends. Codex's request is `features.hooks` false and `notify` empty.
OpenCode's request is `OPENCODE_PURE=1`, project config disabled, and
`OPENCODE_CONFIG`, `OPENCODE_CONFIG_CONTENT`, and `OPENCODE_CONFIG_DIR` unset.
The committed logs are `evidence/codex` and `evidence/opencode`. Claude's log
is not committed until the machine that holds the login runs `run-claude.sh`.
