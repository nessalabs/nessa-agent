import { emptyStrip, type ConversationStrip } from "../model"

export type LocalStrip = ConversationStrip & {
  nextConversationId: number
  nextTurnId: number
}

export function emptyLocalStrip(): LocalStrip {
  return {
    ...emptyStrip(),
    nextConversationId: 1,
    nextTurnId: 1,
  }
}
