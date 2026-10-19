/**
 * The words the share dialog says (nessa_ui#128, #129): each failure kind as
 * a sentence a person reads — never a wire code — by total tables, so a kind
 * the model adds is a type error here rather than a blank line; the roles and
 * presets by name; a grant's one-line summary; and the notes on what a tool
 * policy does not stop.
 */
import { linkedOn } from "../../../model/linked-device"
import type {
  DraftHold,
  ShareGrant,
  SharingLimits,
  ShareRole,
  SharingFailure,
  ToolPolicy,
  ToolPreset,
} from "../../model/sharing"

/** Why a share or a revoke did not happen. */
export const changeFailureCopy: Record<SharingFailure, string> = {
  "not-paired":
    "That device is no longer linked to you, or may not read conversations. Link it again in Settings › Linked devices.",
  invalid:
    "The gateway didn’t accept that grant. Check the tool patterns and hosts, then try again.",
  deleted: "This conversation was deleted.",
  "not-found": "The gateway doesn’t have this conversation.",
  busy: "The gateway is busy. Nothing changed. Try again in a moment.",
  storage: "The gateway couldn’t read or save who this conversation is shared with.",
  audit: "The gateway couldn’t record the change, so it didn’t make it.",
  "not-configured": "This gateway doesn’t run conversations.",
  refused: "This sign-in isn’t allowed to share conversations.",
  "signed-out": "The gateway no longer accepts this window’s sign-in. Sign in again.",
  unenforceable:
    "The gateway can’t hold Drive to its tool policy in this conversation: it needs Ask mode, the agent’s own hooks off, and an agent other than OpenCode.",
  changed:
    "This device’s sharing changed since this list was read, so nothing was changed here. Reading the list again shows what it holds now.",
  other: "The gateway refused this change.",
  unanswered:
    "No answer this window could read came from the gateway, so whether it happened isn’t known. Reading the list again shows where sharing stands.",
}

/** Why the dialog could not read who the conversation is shared with. */
export const readFailureCopy: Record<SharingFailure, string> = {
  "not-paired": "The gateway couldn’t list your linked devices.",
  invalid: "The gateway didn’t accept the request for this conversation’s sharing.",
  deleted: "This conversation was deleted.",
  "not-found": "The gateway doesn’t have this conversation.",
  busy: "The gateway is busy. Try again in a moment.",
  storage: "The gateway couldn’t read who this conversation is shared with.",
  audit: "The gateway couldn’t read who this conversation is shared with.",
  "not-configured": "This gateway doesn’t run conversations.",
  refused: "This sign-in isn’t allowed to share conversations.",
  "signed-out": "The gateway no longer accepts this window’s sign-in. Sign in again.",
  unenforceable: "The gateway couldn’t read who this conversation is shared with.",
  changed: "The gateway couldn’t read who this conversation is shared with.",
  other: "The gateway couldn’t read who this conversation is shared with.",
  unanswered: "No answer this window could read came from the gateway.",
}

/**
 * Why the dialog could not read the linked devices and hosts it offers. The
 * grants it did read stay, and each can still be revoked.
 */
const choicesUnread =
  "The gateway couldn’t list your linked devices and the hosts they could use. You can still revoke a device below."
export const choicesFailureCopy: Record<SharingFailure, string> = {
  "not-paired": choicesUnread,
  invalid: choicesUnread,
  deleted: choicesUnread,
  "not-found": choicesUnread,
  busy: "The gateway is busy and couldn’t list your linked devices. Try again in a moment.",
  storage: choicesUnread,
  audit: choicesUnread,
  "not-configured": choicesUnread,
  refused: "This sign-in isn’t allowed to list linked devices.",
  "signed-out": "The gateway no longer accepts this window’s sign-in. Sign in again.",
  unenforceable: choicesUnread,
  changed: choicesUnread,
  other: choicesUnread,
  unanswered:
    "No answer this window could read came from the gateway about your linked devices.",
}

export const roleCopy: Record<ShareRole, { label: string; description: string }> = {
  read: { label: "Read", description: "Can read and follow this conversation." },
  comment: {
    label: "Comment",
    description: "Can also leave notes in this conversation.",
  },
  drive: {
    label: "Drive",
    description:
      "Can also start, steer and stop turns and answer the agent’s requests, under the tool policy below.",
  },
}

export const presetCopy: Record<
  ToolPreset | "custom",
  { label: string; description: string }
> = {
  read_only_tools: { label: "Read-only tools", description: "Only tools that read." },
  no_shell: { label: "No shell", description: "Every tool except shell commands." },
  ask_everything: {
    label: "Ask every time",
    description: "Every tool, each call asked about.",
  },
  all: {
    label: "All tools",
    description: "Every tool, asked about as the conversation’s approval mode asks.",
  },
  custom: { label: "Custom", description: "Tools you allow and deny by name." },
}

/**
 * What a policy does not stop, said beside the Drive controls (nessa_ui#128).
 * The one note there: what the hosts and the conversation's mode allow is
 * said by the controls and the gateway's refusal (`changeFailureCopy`).
 */
export const driveNote =
  "Calls the agent doesn’t ask about aren’t checked: Claude reads files without asking, and Codex runs commands in its read-only sandbox without asking. A deny pattern doesn’t stop those."

/** A host's list, said in words: "devbox and build", or "nowhere". */
function hostsLine(hosts: readonly string[]): string {
  if (hosts.length === 0) return "commands nowhere"
  if (hosts.length === 1) return `commands on ${hosts[0]}`
  return `commands on ${hosts.slice(0, -1).join(", ")} and ${hosts[hosts.length - 1]}`
}

function policyLine(policy: ToolPolicy): string {
  const tools =
    policy.kind === "preset"
      ? presetCopy[policy.preset].label
      : [
          policy.allow.length ? `allows ${policy.allow.join(", ")}` : null,
          policy.deny.length ? `denies ${policy.deny.join(", ")}` : null,
        ]
          .filter((part) => part !== null)
          .join(", ")
  return `${tools}, ${hostsLine(policy.commandHosts)}`
}

/** A grant in one line: its role, and for Drive its tools and hosts. */
export function grantSummary(grant: ShareGrant): string {
  const role = roleCopy[grant.role].label
  return grant.policy ? `${role} · ${policyLine(grant.policy)}` : role
}

/** A linked device's line, in Settings' words, and the end of its id to tell two linked the same day apart. */
export function deviceDetail(credentialId: string, issuedAt: number): string {
  return `Linked ${linkedOn(issuedAt)} · ID …${credentialId.slice(-6)}`
}

/** Why the Share button cannot send yet. */
export function draftReasonCopy(reason: DraftHold, limits: SharingLimits): string {
  const said: Record<DraftHold, string> = {
    "no-target": "Choose a device to share with.",
    changed:
      "This device’s sharing changed since this draft began. Start again from what it holds now.",
    "no-pattern": "Name at least one tool to allow or deny.",
    "too-many-patterns": `At most ${limits.patterns} tools in each of Allow and Deny.`,
    "too-many-hosts": `At most ${limits.hosts} hosts.`,
  }
  return said[reason]
}

/** Why the form waits on a change on its way (one change at a time), by whose sharing it changes. */
export const waitingCopy = {
  "this-session": "Waiting for your change to this conversation’s sharing to finish.",
  "another-session":
    "Waiting for a change to another conversation’s sharing to finish. One change is made at a time.",
} as const

export const doneCopy = { share: "Shared.", revoke: "No longer shared." } as const
