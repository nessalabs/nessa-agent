import type { ConversationView } from "../generated/product.js"

declare const executionTarget: unique symbol

/** A captured conversation and execution, not current authorization. */
export type ExecutionTarget = {
  readonly conversationId: string
  readonly executionId: string
  readonly [executionTarget]: true
}
const issued = new WeakSet<ExecutionTarget>()

/** @internal Only preparation and enclosing views issue execution targets. */
export function issueExecutionTarget(conversationId: string, executionId: string) {
  const target = Object.freeze({ conversationId, executionId }) as ExecutionTarget
  issued.add(target)
  return target
}

/** Capture execution identities in message/queue order from a whole view. */
export function executionTargets(view: {
  readonly conversationId: ConversationView["conversationId"]
  readonly messages: readonly { readonly executionId: string }[]
  readonly pending: readonly { readonly executionId: string }[]
}): readonly ExecutionTarget[] {
  const conversationId = view.conversationId
  return Object.freeze(
    [...new Set([...view.messages, ...view.pending].map((turn) => turn.executionId))].map(
      (executionId) => issueExecutionTarget(conversationId, executionId),
    ),
  )
}

/** Check an issued target before routing a captured local action. */
export function executionTargetConversation(target: ExecutionTarget): string {
  return executionFields(target).conversationId
}

/** @internal Reject copied targets even when TypeScript was bypassed. */
export function executionFields(target: ExecutionTarget) {
  if (!issued.has(target))
    throw new TypeError("Use an execution target from a view or prepared submission")
  return { conversationId: target.conversationId, executionId: target.executionId }
}
