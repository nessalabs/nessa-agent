/** One choice an agent offers, and what happens if it is picked. */
export interface OwnerAskChoiceView {
  label: string
  consequence: string
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
}
