/**
 * Sharing a session with a linked device (nessa_ui#128, #129): who it is
 * shared with, what each may do, and the room a Drive grant's tool policy
 * has for the hosts it may run commands on. The gateway owns every rule
 * here — whether a device may be granted, what a policy may hold — and
 * refuses past them; this model holds what it answered, and budgets the
 * hosts a person may pick against the host bound the gateway's schema
 * publishes (`SharingLimits`), never numbers of its own.
 *
 * Words a person reads for each failure are the UI's
 * (`ui/share/share-copy.ts`); state keeps the kind.
 */
import type { LinkedDevice } from "../../model/linked-device"

/** What a grant lets its holder do; each allows what the ones before it do. */
export type ShareRole = "read" | "comment" | "drive"

/** A named tool policy. */
export type ToolPreset = "read_only_tools" | "no_shell" | "ask_everything" | "all"

/**
 * The tools a Drive grant's turns may call, and the hosts they may run
 * commands on. Hosts not named run nothing: a grant naming none runs
 * commands nowhere.
 */
export type ToolPolicy =
  | {
      readonly kind: "preset"
      readonly preset: ToolPreset
      readonly commandHosts: readonly string[]
    }
  | {
      readonly kind: "rules"
      readonly allow: readonly string[]
      readonly deny: readonly string[]
      readonly commandHosts: readonly string[]
    }

/** One grant on the session, as the gateway lists it. */
export interface ShareGrant {
  readonly credentialId: string
  readonly role: ShareRole
  /** Present exactly on a Drive grant. */
  readonly policy?: ToolPolicy
  /**
   * Moves each time the owner changes this grant's role or policy, and only
   * ever up: a grant made after a revoke takes one past every one before.
   * A change built on the grant names it, and the gateway refuses the change
   * if the grant has moved since (`changed`).
   */
  readonly policyRevision: number
  readonly grantedAtMs: number
}

/** A linked device the session may be shared with. */
export type ShareTarget = LinkedDevice

/**
 * A tool policy's bounds as the gateway's schema publishes them: each of
 * allow and deny holds at most `patterns`; at most `hosts` hosts of at most
 * `hostBytes` UTF-8 bytes. Hosts are apart from the patterns.
 */
export interface SharingLimits {
  readonly patterns: number
  readonly hosts: number
  readonly hostBytes: number
}

/**
 * What the form offers, read beside the grants but apart from them, so a
 * failure here leaves the grant list and its Revoke in place.
 */
export interface SharingChoices {
  readonly targets: readonly ShareTarget[]
  /**
   * The hosts the owner's own turns may run commands on (`agents.list`'s
   * `commandHosts`): the only ones a grant could use.
   */
  readonly commandHosts: readonly string[]
  readonly limits: SharingLimits
}

/** A grant to make, or the one a device holds to replace. */
export interface ShareRequest {
  readonly credentialId: string
  readonly role: ShareRole
  /** Present exactly when `role` is drive. */
  readonly policy?: ToolPolicy
  /**
   * The policy revision of the grant this request was built on, 0 when the
   * device held none (a revision starts at 1). The gateway makes the change
   * only while the device still holds exactly that, so a grant made or
   * changed since — a first grant from another window too — is refused.
   */
  readonly expectedPolicyRevision: number
}

/**
 * Why a share, a revoke or a read did not happen, by kind.
 *
 * `not-paired`: the device is not an active linked device of this owner
 * that may read. `invalid`: the gateway refused what was asked — a policy
 * past its bounds, or a conversation past its grants. `deleted`: the
 * conversation was deleted. `not-found`: the gateway holds no such
 * conversation of this caller. `busy`: not now; nothing changed. `storage`:
 * the gateway could not read or keep the conversation's grants. `audit`:
 * the record of the change could not be kept. `not-configured`: the gateway
 * runs no conversations. `refused`: this sign-in may not share.
 * `signed-out`: the gateway no longer accepts this window's sign-in.
 * `unenforceable`: a Drive grant's turns could not be held to its policy
 * here — the conversation is not in Ask mode, its agent's own hooks are on,
 * or its agent is OpenCode. `changed`: the device's grant moved since the
 * revision the change was built on — in another window, or by this
 * window's own change whose answer, or the read after it, was lost — so
 * nothing was changed. Its words name no cause. `other`: a
 * refusal this dialog has no words of its own for. `unanswered`:
 * no answer came, so whether it happened is not known — the list, read
 * again, says where it stands.
 */
export type SharingFailure =
  | "not-paired"
  | "invalid"
  | "deleted"
  | "not-found"
  | "busy"
  | "storage"
  | "audit"
  | "not-configured"
  | "refused"
  | "signed-out"
  | "unenforceable"
  | "changed"
  | "other"
  | "unanswered"

/** The patterns a custom policy names. A preset names none. */
export function patternCount(policy: ToolPolicy): number {
  return policy.kind === "rules" ? policy.allow.length + policy.deny.length : 0
}

const utf8 = new TextEncoder()

/** Whether a host's name fits a grant: an SSH destination can be longer than one may hold. */
export function hostFits(limits: SharingLimits, host: string): boolean {
  return utf8.encode(host).byteLength <= limits.hostBytes
}

/**
 * The patterns a person typed, between commas or one per line, each with
 * the spaces around it taken off and blanks dropped. What a pattern may be
 * is the gateway's to say; it refuses one it does not take.
 *
 * A pattern may itself hold a comma, which typed text cannot tell from a
 * separator. Those in `kept` — the patterns of the grant being changed —
 * are read back whole wherever their text still stands between separators,
 * so editing the field around one leaves it as it was. A new pattern with a
 * comma cannot be typed here.
 */
export function patternsFrom(text: string, kept: readonly string[] = []): string[] {
  const pieces = text.split(/[\n,]/)
  const joined = kept.filter((pattern) => pattern.includes(","))
  const longest = Math.max(1, ...joined.map((pattern) => pattern.split(",").length))
  const patterns: string[] = []
  for (let at = 0; at < pieces.length;) {
    let taken = 1
    for (let span = Math.min(longest, pieces.length - at); span > 1; span--) {
      const whole = pieces
        .slice(at, at + span)
        .join(",")
        .trim()
      if (joined.includes(whole)) {
        taken = span
        break
      }
    }
    const pattern = pieces
      .slice(at, at + taken)
      .join(",")
      .trim()
    if (pattern.length > 0) patterns.push(pattern)
    at += taken
  }
  return patterns
}

/** The grant the dialog will make, as the person has set it. */
export interface ShareDraft {
  readonly credentialId: string | null
  /**
   * The policy revision of the grant the draft builds on, or `null` when the
   * device held none: from the list when the draft started from it, and from
   * the gateway's answer to this window's own change to it, never from a
   * later read. It is sent as the expected revision; when the list reads
   * otherwise, the grant changed under the draft (`changed`).
   */
  readonly basis: number | null
  readonly role: ShareRole
  /** `custom` is allow and deny patterns rather than a preset. */
  readonly preset: ToolPreset | "custom"
  /** The patterns as typed, one field each. */
  readonly allow: string
  readonly deny: string
  /** The patterns of the grant the draft started from (`patternsFrom`'s `kept`). */
  readonly kept: readonly string[]
  /**
   * The hosts checked, in the list's order. A host the grant holds that the
   * gateway does not offer now is kept, and shown, until it is unchecked:
   * a host offline for a moment is not taken out of a grant by a change to
   * its tools.
   */
  readonly hosts: readonly string[]
}

/** A new draft for a device holding no grant: Read, the read-only preset for Drive, no host. */
export function emptyDraft(credentialId: string | null): ShareDraft {
  return {
    credentialId,
    basis: null,
    role: "read",
    preset: "read_only_tools",
    allow: "",
    deny: "",
    kept: [],
    hosts: [],
  }
}

/** A custom draft's patterns, each list as read from its field. */
function draftPatterns(draft: ShareDraft): { allow: string[]; deny: string[] } {
  return {
    allow: patternsFrom(draft.allow, draft.kept),
    deny: patternsFrom(draft.deny, draft.kept),
  }
}

/** Why a draft cannot be sent yet. */
export type DraftHold =
  "no-target" | "changed" | "no-pattern" | "too-many-patterns" | "too-many-hosts"

/**
 * The grant a draft asks for, or why it cannot be sent yet: no device; the
 * device's grant changed since the draft started from it; a custom policy
 * naming no pattern, or more in one list than the schema allows; more hosts
 * than the host bound allows. Nothing else is checked here: the gateway
 * says the rest.
 */
export function requestFrom(
  draft: ShareDraft,
  grants: readonly ShareGrant[],
  limits: SharingLimits,
):
  | { readonly ok: true; readonly request: ShareRequest }
  | { readonly ok: false; readonly reason: DraftHold } {
  if (draft.credentialId === null) return { ok: false, reason: "no-target" }
  const held = grants.find((grant) => grant.credentialId === draft.credentialId)
  if ((held?.policyRevision ?? null) !== draft.basis)
    return { ok: false, reason: "changed" }
  const expected = { expectedPolicyRevision: draft.basis ?? 0 }
  if (draft.role !== "drive")
    return {
      ok: true,
      request: { credentialId: draft.credentialId, role: draft.role, ...expected },
    }
  const commandHosts = [...draft.hosts]
  const policy: ToolPolicy =
    draft.preset === "custom"
      ? { kind: "rules", ...draftPatterns(draft), commandHosts }
      : { kind: "preset", preset: draft.preset, commandHosts }
  if (policy.kind === "rules") {
    if (patternCount(policy) === 0) return { ok: false, reason: "no-pattern" }
    if (Math.max(policy.allow.length, policy.deny.length) > limits.patterns)
      return { ok: false, reason: "too-many-patterns" }
  }
  if (commandHosts.length > limits.hosts) return { ok: false, reason: "too-many-hosts" }
  return {
    ok: true,
    request: { credentialId: draft.credentialId, role: "drive", policy, ...expected },
  }
}

/** A draft that starts from what a device holds now, so a change begins where it stands. */
export function draftFromGrant(grant: ShareGrant): ShareDraft {
  const base: ShareDraft = {
    ...emptyDraft(grant.credentialId),
    basis: grant.policyRevision,
    role: grant.role,
  }
  const policy = grant.policy
  if (!policy) return base
  return policy.kind === "preset"
    ? { ...base, preset: policy.preset, hosts: policy.commandHosts }
    : {
        ...base,
        preset: "custom",
        // One line each: the fields are single-line inputs, which drop a newline.
        allow: policy.allow.join(", "),
        deny: policy.deny.join(", "),
        kept: [...policy.allow, ...policy.deny],
        hosts: policy.commandHosts,
      }
}

/**
 * The draft once this window's own change to device `changed` was made, as
 * the gateway answered it: it builds on the revision that answer named —
 * `held`, or `null` after a revoke — and keeps what the person set. A
 * change refused or unanswered names nothing, so it moves no draft: what was
 * made, if anything, shows as changed under it, with Start Again. Another
 * device's change leaves the draft as it is.
 */
export function draftSettled(
  draft: ShareDraft,
  changed: string,
  held: number | null,
): ShareDraft {
  return draft.credentialId === changed ? { ...draft, basis: held } : draft
}

/** The draft started again from what its device holds: Start Again, for a draft changed under it. */
export function draftRestarted(
  draft: ShareDraft,
  grants: readonly ShareGrant[],
): ShareDraft {
  const device = draft.credentialId
  if (device === null) return draft
  const grant = grants.find((each) => each.credentialId === device)
  return grant ? draftFromGrant(grant) : emptyDraft(device)
}
