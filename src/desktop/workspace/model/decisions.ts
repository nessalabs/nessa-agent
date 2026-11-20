/**
 * The decisions an agent asked the person in one chat (`ask_owner`), and
 * what became of each: still waiting, decided by the person's message, or
 * replaced by a newer question before anyone answered. The gateway keeps
 * them (`conversation.decisions`); this is how the window says them.
 */
import type { OwnerAsk } from "./workspace-index"

export type DecisionOutcome =
  | { readonly state: "waiting" }
  | {
      readonly state: "decided"
      /** What the person wrote, cut short by the gateway. */
      readonly text: string
      /** Index into the ask's choices of the one the message names; absent for the person's own words. */
      readonly choice?: number
      /** Auto-pick sent the recommended choice because nobody answered in time. */
      readonly auto?: true
      readonly atMs: number
    }
  | { readonly state: "replaced"; readonly atMs: number }

export interface Decision {
  readonly ask: OwnerAsk
  readonly askedAtMs: number
  readonly outcome: DecisionOutcome
  /**
   * While it waits, auto-pick's place: when the gateway sends the recommended
   * choice, or that the person paused it. Absent when it will not pick: the
   * agent did not allow it, or auto-pick is off in Settings.
   */
  readonly autoPick?:
    { readonly state: "due"; readonly atMs: number } | { readonly state: "paused" }
}

export type DecisionState = DecisionOutcome["state"]

/** The decisions dialog's filter: what waits on the person, everything, or what is settled. */
export type DecisionsFilter = "waiting" | "all" | "decided"

/** How many decisions Chat context lists before "See all". */
export const decisionsShown = 3

/**
 * Waiting first, then as the gateway listed them: newest first, in the
 * order they were asked, which a clock that stepped back cannot reorder.
 */
export function decisionsInOrder(decisions: readonly Decision[]): readonly Decision[] {
  return [
    ...decisions.filter((decision) => decision.outcome.state === "waiting"),
    ...decisions.filter((decision) => decision.outcome.state !== "waiting"),
  ]
}

/** The decisions a filter keeps; "decided" also keeps the replaced ones, which are settled too. */
export function decisionsFiltered(
  decisions: readonly Decision[],
  filter: DecisionsFilter,
): readonly Decision[] {
  if (filter === "all") return decisions
  return decisions.filter((decision) =>
    filter === "waiting"
      ? decision.outcome.state === "waiting"
      : decision.outcome.state !== "waiting",
  )
}

/**
 * Where the dialog opens: on To decide when the decision it opens on waits,
 * and on All otherwise, so the chosen one is always in the list.
 */
export function filterFor(decision: Decision | undefined): DecisionsFilter {
  return decision?.outcome.state === "waiting" ? "waiting" : "all"
}

/** The chosen choice's label, or "Your own words". */
export function answerLabel(decision: Decision): string | undefined {
  const outcome = decision.outcome
  if (outcome.state !== "decided") return undefined
  const choice =
    outcome.choice === undefined ? undefined : decision.ask.choices[outcome.choice]
  return choice?.label ?? "Your own words"
}

/** One short line under a decision's question: where it stands. */
export function decisionLine(decision: Decision, now: number): string {
  const outcome = decision.outcome
  switch (outcome.state) {
    case "waiting": {
      const pick = decision.autoPick
      if (pick?.state === "due")
        return `Waiting on you · auto-picks in ${left(pick.atMs, now)}`
      if (pick?.state === "paused") return "Waiting on you · auto-pick paused"
      return `Waiting on you · asked ${ago(decision.askedAtMs, now)}`
    }
    case "decided":
      return `${outcome.auto ? "Auto-picked " : ""}${answerLabel(decision)} · ${ago(outcome.atMs, now)}`
    case "replaced":
      return "Replaced by a newer question"
  }
}

/** The state as a label: Waiting on you, Decided, Auto-picked, Replaced. */
export function stateLabel(outcome: DecisionOutcome): string {
  return outcome.state === "waiting"
    ? "Waiting on you"
    : outcome.state === "decided"
      ? outcome.auto
        ? "Auto-picked"
        : "Decided"
      : "Replaced"
}

/** Time until `atMs`, the way a countdown says it: 4:05, 59:59, 2h 5m; 0:00 once it has passed. */
export function left(atMs: number, now: number): string {
  const seconds = Math.ceil(Math.max(0, atMs - now) / 1000)
  if (seconds >= 3600)
    return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`
}

/** How long ago, the way a list says it: just now, 4m ago, 2h ago, 3d ago. */
export function ago(atMs: number, now: number): string {
  const minutes = Math.floor(Math.max(0, now - atMs) / 60_000)
  if (minutes < 1) return "just now"
  if (minutes < 60) return `${minutes}m ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h ago`
  return `${Math.floor(hours / 24)}d ago`
}
