# 768. Native harness hooks are a per-harness choice

## Purpose

A person can let Claude Code or Codex run its own hooks. The choice is off
by default, so today's suppression request stays unless they turn it on.
OpenCode is out of this decision and stays suppressed.

- **Date:** Oct 11, 2026
- **Status:** accepted

## Context

Nessa does not import or re-run a third-party harness's hooks (#135). Those
harnesses may run their own hooks when the person chooses, and Nessa-owned
hooks are for Nessa's own agent later. Claude and Codex suppress differently,
and each still has a residual path Nessa cannot close: Claude managed hooks
fire even with `disableAllHooks`, and Codex legacy managed configuration and
MDM can override `features.hooks: false` and an empty `notify`. A conversation
runs one harness (#142).

## Decision

The setting is per harness, for Claude and Codex only. Off, the default,
keeps today's suppression request. On, that harness's process start omits the
suppression request. Claude then loads the setting sources the Claude CLI
loads: user, project, and local. That also applies the repo's `.claude`
settings, including its allow rules and startup hooks, so turn it on only for
projects you trust. Codex omits `features.hooks` and `notify` and does not
send `features.hooks: true`. It does not send an enable flag. The negotiated
suppression fact stays what it is today. The choice is stored beside the
gateway, recorded with the caller, and shown on the conversation and in
Settings. Each Claude or Codex process start reads the current switch once
and uses that value for the profile and, for Codex, the launch environment.
Applies the next time each conversation's agent starts, including a
conversation that is already open. A process already running keeps the choice
it started with. The conversation view shows that start's value, and when it
differs from the current switch says the new value applies at the next start.
Hooks can approve, deny, or change a tool call and its result without Nessa
asking, and can run commands when the agent starts. With hooks on, recorded
input and output are what the model proposed or the tool returned, and a hook
may have changed or approved them.

## Ordering

One command is one harness and one request id. The other harness is a
different command. The rows are the orders the store implements.
`a_change_records_the_caller_and_a_retry_keeps_one_record_per_phase`,
`a_failed_requested_record_leaves_the_choice_off`,
`a_failed_outcome_record_reverts_the_choice`,
`a_file_without_an_outcome_loads_as_written`,
`a_retry_after_a_failed_outcome_keeps_the_first_observation`, and
`a_reused_request_id_for_a_different_choice_fails`, and
`a_failed_revert_leaves_the_flag_with_the_published_file` are those rows.
The launch row is `claude_native_hook_runs_when_the_launch_omits_suppression`
and `codex_native_hook_runs_when_the_launch_omits_suppression`. The next start
of a conversation already open is
`turning_hooks_off_suppresses_an_existing_claude_conversations_next_launch`
and
`turning_hooks_off_suppresses_an_existing_codex_conversations_next_launch`.

| Now | What arrives | What follows |
| --- | --- | --- |
| Off, no records | The caller asks for on | Requested record, then the file and the flag, then the outcome record |
| No requested record | The requested record fails | The file is not written. The flag stays. The command fails |
| Requested record written, file still off | The outcome record fails | The file and the flag stay off. The command fails |
| Requested record written, file and flag on | The outcome record fails and the revert write succeeds | The file and the flag return to the choice this call started from. The command fails |
| File published | The revert write fails | The flag stays with that file. The command fails. The outcome record is absent |
| File written, outcome absent | The process starts | The start loads the file |
| Both records written for this choice | The same caller, harness, and request id | The command succeeds. The records stay as first written, including the observation time |
| Outcome written for a different choice | The same request id | The command fails. The file is unchanged |
| The choice is on | That harness's process starts | Claude sends setting sources user, project, and local and omits `disableAllHooks`. Codex omits `features.hooks` and `notify`. A process already running is not restarted |

## Alternatives considered

One global switch lost because OpenCode must stay suppressed, and because
Claude and Codex do not share a suppression request or a residual risk. A
global on would either start OpenCode's hooks or hide that it cannot. Leaving
the choice inside each provider, with no shared store, lost because a
credentialed Claude provider is rebuilt on every open and the details panel
needs one current value.

## Consequences

Each process start of Claude or Codex follows the stored choice, including
the next start of a conversation already open. A process already running is
unchanged. OpenCode's launch environment is untouched.
Managed and MDM hooks can still run when the choice is off, and allowing
hooks does not prove a hook ran. A crash after the file is written and before
the outcome record is written leaves the file ahead of the audit; the next
start loads the file.
