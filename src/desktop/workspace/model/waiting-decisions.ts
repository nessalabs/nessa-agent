/**
 * The decisions waiting on the person across every chat: the queue the
 * Decisions view works through. Each is a chat whose list row carries an
 * agent's ask (`SessionSummary.ask`), so the gateway's list is the one
 * authority: a decision leaves the queue when the list says nothing is asked
 * there any more, never because the window guessed it was answered.
 *
 * An access request is not here: it is answered on its red card in the chat,
 * where Approve raises the chat's access first.
 */
import {
  agentName,
  agentOf,
  type OwnerAsk,
  type OwnerAskChoice,
  type SessionSummary,
} from "./workspace-index"

export interface WaitingDecision {
  readonly sessionId: string
  /** The chat's title, which names what the decision blocks. */
  readonly title: string
  /** The asking agent's name as people say it. */
  readonly agent: string
  /** What the agent is doing, in the source's words, when it said. */
  readonly doing?: string
  readonly ask: OwnerAsk
  /**
   * When the chat last changed. While it waits on the person nothing else
   * happens in it, so this is when the agent asked.
   */
  readonly askedAtMs: number
}

/**
 * Every waiting decision, the longest waiting first: an ask that has waited
 * longest has held its agent up longest. Ties keep the list's order.
 */
export function waitingDecisions(
  sessions: readonly SessionSummary[],
): readonly WaitingDecision[] {
  return sessions
    .filter(
      (session): session is SessionSummary & { ask: OwnerAsk } =>
        session.ask !== undefined && session.ask.access === undefined,
    )
    .map((session) => ({
      sessionId: session.id,
      title: session.title,
      agent: agentName(agentOf(session.model)),
      ...(session.now?.trim() ? { doing: session.now.trim() } : {}),
      ask: session.ask,
      askedAtMs: session.updatedAt,
    }))
    .sort((a, b) => a.askedAtMs - b.askedAtMs)
}

/** The waiting decisions under their agent, agents in the order they first wait. */
export function decisionsByAgent(
  decisions: readonly WaitingDecision[],
): readonly { readonly agent: string; readonly decisions: readonly WaitingDecision[] }[] {
  const groups = new Map<string, WaitingDecision[]>()
  for (const decision of decisions) {
    const group = groups.get(decision.agent)
    if (group) group.push(decision)
    else groups.set(decision.agent, [decision])
  }
  return [...groups].map(([agent, held]) => ({ agent, decisions: held }))
}

/** The choice the agent suggests, when its index names one. */
export function suggested(ask: OwnerAsk): OwnerAskChoice | undefined {
  return ask.choices[ask.recommended]
}

/**
 * What the person's answer says, as their own message: a choice's label, or
 * the label, ". " and a note, which the gateway still reads as that choice;
 * or, for an answer in their own words, those words alone. Undefined when
 * there is nothing to send.
 */
export function answerText(
  ask: OwnerAsk,
  answer: { readonly choice: number; readonly note?: string } | { readonly own: string },
): string | undefined {
  if ("own" in answer) return answer.own.trim() || undefined
  const choice = ask.choices[answer.choice]
  if (!choice) return undefined
  const note = answer.note?.trim()
  return note ? `${choice.label}. ${note}` : choice.label
}

/**
 * Whether Accept all starts with a decision ticked: only when the suggested
 * choice is easy to undo. One that is costly to take back, can't be undone,
 * or whose agent did not say, is left for the person to tick, since a slip
 * in a bulk answer should only ever cost an undo.
 */
export function acceptedByDefault(decision: WaitingDecision): boolean {
  return suggested(decision.ask)?.undo === "easy"
}

/**
 * The facts a decision's options can be compared on, each only when the
 * agent said it for some choice: a row the same blank on every column tells
 * the person nothing. Undo shows only when it differs between the choices,
 * as on the chat's card (`undoDiffers`).
 */
export type OptionFact = "consequence" | "pros" | "cons" | "effort" | "undo"

export function optionFacts(ask: OwnerAsk): readonly OptionFact[] {
  const some = (has: (choice: OwnerAskChoice) => boolean) => ask.choices.some(has)
  const undos = ask.choices.map((choice) => choice.undo)
  const facts: OptionFact[] = ["consequence"]
  if (some((choice) => (choice.pros?.length ?? 0) > 0)) facts.push("pros")
  if (some((choice) => (choice.cons?.length ?? 0) > 0)) facts.push("cons")
  if (some((choice) => choice.effort !== undefined)) facts.push("effort")
  if (undos.every((undo) => undo !== undefined) && new Set(undos).size > 1)
    facts.push("undo")
  return facts
}
