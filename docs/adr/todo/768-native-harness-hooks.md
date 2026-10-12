# 768. Native harness hooks are a per-harness choice

## Purpose

A person can let Claude Code or Codex run its own hooks. The choice is off
by default, so today's suppression request stays unless they turn it on.
OpenCode is out of this decision and stays suppressed.

- **Date:** 2026-10-12
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
keeps today's suppression request. On, the next launch of that harness omits
the request. It does not send an enable flag. The negotiated suppression fact
stays what it is today. The choice is stored beside the gateway, recorded
with the caller, and shown on the conversation and in Settings. When it is
on, a hook can deny or change a tool call before Nessa's permission flow
sees it.

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
and `codex_native_hook_runs_when_the_launch_omits_suppression`.

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
| The choice is on | That harness launches | The suppression keys are omitted. A process already running is not restarted |

## Alternatives considered

One global switch lost because OpenCode must stay suppressed, and because
Claude and Codex do not share a suppression request or a residual risk. A
global on would either start OpenCode's hooks or hide that it cannot. Leaving
the choice inside each provider, with no shared store, lost because a
credentialed Claude provider is rebuilt on every open and the details panel
needs one current value.

## Consequences

The next launch of Claude or Codex follows the stored choice. A process
already running is unchanged. OpenCode's launch environment is untouched.
Managed and MDM hooks can still run when the choice is off, and allowing
hooks does not prove a hook ran. A crash after the file is written and before
the outcome record is written leaves the file ahead of the audit; the next
start loads the file.
