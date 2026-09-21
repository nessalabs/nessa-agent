import { ConversationErrorCode } from "../generated/product.js"

/**
 * The conversation rejection code this build knows by that name, or `undefined`
 * for any other string.
 *
 * A gateway that returns a code this build does not know is not reinterpreted as
 * one it does: the caller gets no typed code and keeps the original cause, which
 * is what an answer nobody here has a meaning for deserves. Use it wherever a
 * raw wire code has to be narrowed before it can be branched on — including
 * before it indexes a lookup table, since an arbitrary string such as
 * `"constructor"` is a member of every object. `NessaConversationMutationError`
 * and `NessaConversationControlError` apply it to their own causes; a caller
 * needs it for `conversation.read`, which rejects with the underlying
 * `NessaRpcError`, whose `code` is a plain string until this narrows it.
 *
 * @param code - The `code` of a `type: "res"` error frame, as it arrived.
 * @returns The matching {@link ConversationErrorCode}, or `undefined`.
 */
export const conversationErrorCode = (code: string): ConversationErrorCode | undefined =>
  (Object.values(ConversationErrorCode) as string[]).includes(code)
    ? (code as ConversationErrorCode)
    : undefined
