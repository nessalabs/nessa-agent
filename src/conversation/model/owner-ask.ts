/** One choice an agent offers, and what happens if it is picked. */
export interface OwnerAskChoiceView {
  label: string
  consequence: string
  /** How hard it is to take back, when the agent said. */
  undo?: "easy" | "costly" | "no"
  /** How much work it is, when the agent said. */
  effort?: "small" | "medium" | "large"
  /** Up to two lines for it, and against it. */
  pros?: string[]
  cons?: string[]
}

/**
 * A decision an agent asked the person (`ask_owner`): a short question, two
 * to four choices, and the one the agent recommends with why. The person's
 * next message in the conversation is the answer, a choice's label or
 * anything else they write.
 */
export interface OwnerAskView {
  /** New for every ask: a later ask in the same conversation is a new one. */
  askId: string
  question: string
  choices: OwnerAskChoiceView[]
  /** Index into `choices` of the one the agent recommends. */
  recommended: number
  reason: string
  /** One line under the question with what it is about. */
  summary?: string
  /** Whether the agent let the recommended choice be picked for the person. */
  autoPick?: "allowed" | "never"
}

/** A choice as a card reads it: nothing in it changed. */
export type OwnerAskChoiceShown = Readonly<Omit<OwnerAskChoiceView, "pros" | "cons">> & {
  readonly pros?: readonly string[]
  readonly cons?: readonly string[]
}

/**
 * An ask as a card reads it: the fields of {@link OwnerAskView}, none of them
 * changed, so a read-only ask from any store can be shown.
 */
export type OwnerAskShown = Readonly<Omit<OwnerAskView, "choices">> & {
  readonly choices: readonly OwnerAskChoiceShown[]
}

/** How a choice's undo reads on a card. */
export const undoLabels = {
  easy: "Easy to undo",
  costly: "Costly to undo",
  no: "Can't undo",
} as const satisfies Record<NonNullable<OwnerAskChoiceView["undo"]>, string>

/**
 * Whether a card says how hard each choice is to undo: only when the agent
 * said so for every choice and they differ, since a line that is the same on
 * every choice tells the person nothing about which to pick.
 */
export function undoDiffers(ask: OwnerAskShown): boolean {
  const undos = ask.choices.map((choice) => choice.undo)
  return undos.every((undo) => undo !== undefined) && new Set(undos).size > 1
}
