import type { ConversationEffects } from "../../application/ports"
import type { OwnerAskView } from "../../model/owner-ask"

/** A decision an agent asked the person, in the conversation the list names. */
export interface WaitingAsk {
  serverConversationId: string
  /** The conversation's title as the list has it, null before it has one. */
  title: string | null
  ask: OwnerAskView
}

/**
 * Follows the gateway's list for the decisions agents asked the person
 * (`ask_owner`) and tells each set the list says is waiting. The list row is
 * the one place an ask is kept: the person's next message in its conversation
 * removes it, and the next list says so. Returns the stop.
 *
 * Its own follow, so a panel tells an ask whether or not its Messages list is
 * open. A failed read is retried by the list follow; until then the last set
 * stands.
 */
export const followOwnerAsks =
  (told: (asks: WaitingAsk[]) => void) =>
  (
    _dispatch: unknown,
    _getState: unknown,
    extra: { conversation: ConversationEffects },
  ): (() => void) =>
    extra.conversation.followList(false, {
      list(list) {
        told(
          list.conversations.flatMap((row) =>
            row.ask
              ? [
                  {
                    serverConversationId: row.conversationId,
                    title: row.title,
                    ask: row.ask,
                  },
                ]
              : [],
          ),
        )
      },
      failed(reason, cause) {
        console.warn("[nessa] asked decisions were not refreshed", reason, cause)
      },
    })
