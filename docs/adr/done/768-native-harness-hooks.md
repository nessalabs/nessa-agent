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
`a_reused_request_id_for_a_different_choice_fails`,
`a_failed_revert_leaves_the_flag_with_the_published_file`,
`a_choice_renamed_into_place_is_in_effect_when_its_directory_sync_fails`,
`a_readable_outcome_record_keeps_the_change_it_records`,
`a_process_start_reads_the_earlier_choice_until_the_outcome_is_recorded`,
`a_harness_named_twice_is_off_whichever_value_comes_last`,
`a_choice_file_larger_than_any_choice_loads_as_off`, and
`a_retry_is_not_acknowledged_until_the_outcome_records_directory_syncs` are those rows.
The launch row is `claude_native_hook_runs_when_the_launch_omits_suppression`
and `codex_native_hook_runs_when_the_launch_omits_suppression`. The next start
of a conversation already open is
`turning_hooks_off_suppresses_an_existing_claude_conversations_next_launch`
and
`turning_hooks_off_suppresses_an_existing_codex_conversations_next_launch`.

| Now | What arrives | What follows |
| --- | --- | --- |
| Off, no records | The caller asks for on | Requested record, then the file, then the outcome record, then the flag. A process start reads the earlier choice until the outcome is recorded |
| No requested record | The requested record fails | The file is not written. The flag stays. The command fails |
| Requested record written, file still off | The outcome record fails | The file and the flag stay off. The command fails |
| Requested record written, file on, flag still off | The outcome record fails and the revert write succeeds | The file returns to the choice this call started from, and the flag never moved. The command fails |
| File published | The revert write fails | The flag stays with that file. The command fails. The outcome record is absent |
| Requested record written | The file is renamed into place and its directory sync fails | The outcome is recorded and the flag follows the file. The command reports the storage failure. A retry succeeds once the directory syncs, and reports the storage failure until then |
| File and flag on | The outcome record is readable but reports a failure | Nothing is reverted, because the record says the change took effect. The command fails. A retry succeeds once the record's directory syncs, and reports the audit failure until then |
| File written, outcome absent | The process starts | The start loads the file |
| The file is not one JSON object, names a harness twice, or is larger than any choice | The gateway starts | That file loads as off and the gateway keeps starting |
| Both records written for this choice | The same caller, harness, and request id | The command succeeds. The records stay as first written, including the observation time |
| Outcome written for a different choice | The same request id | The command fails. The file is unchanged |
| The choice is on | That harness's process starts | Claude sends setting sources user, project, and local and omits `disableAllHooks`. Codex omits `features.hooks` and `notify`. A process already running is not restarted |
| The choice is on | A session is deleted | The deletion process keeps hooks suppressed. Deleting is Nessa's housekeeping, not a run of the agent |
| Claude or Codex starts a process | Before any request reaches it | The launch record is written with the choice, the cause (open or the runtime's restart), and whether saved context was resumed. No hook can run before it: the earliest hook, SessionStart, needs a session, and no session is requested until the record is durable |
| A process start runs a tool call | The call's events are saved | Each tool event carries that start's choice, so a reader after a restart shows the choice the call ran under. An event saved without one shows none |

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
unchanged. OpenCode's launch environment is untouched. The agent catalog
holds the choice once, at its top level; agent rows do not repeat it.
Managed and MDM hooks can still run when the choice is off, and allowing
hooks does not prove a hook ran. A crash after the file is written and before
the outcome record is written leaves the file ahead of the audit; the next
start loads the file.
