import type { ConversationView } from "../generated/product.js"

declare const permissionTarget: unique symbol

/**
 * One review captured from an enclosing conversation projection by
 * {@link permissionTargets}. This is read-only intent, not authorization.
 * Keep it for one deliberate answer/cancel; reread after an uncertain outcome.
 * It is process-local: JSON/spread copies must be reminted from a fresh view.
 */
export type PermissionTarget = {
  readonly conversationId: string
  /** The key the view listed the review under; the answer names it. */
  readonly review: string
  readonly executionId: string
  readonly permissionId: string
  readonly [permissionTarget]: true
}

type PermissionView = {
  readonly conversationId: ConversationView["conversationId"]
  readonly permissions: readonly {
    readonly review: string
    readonly executionId: string
    readonly permissionId: string
    readonly options: readonly { readonly id: string }[]
  }[]
}

// Weak ownership retains no targets after their callers let them go. Identity
// membership rejects copied/forged objects even when TypeScript is bypassed.
const offered = new WeakMap<PermissionTarget, ReadonlySet<string>>()

/**
 * Capture each review's conversation, execution and choices together, in view
 * order. The input is a validated gateway view or its trusted local projection;
 * this factory does not authenticate a fabricated view. The gateway checks
 * current access and pending state when applying either control.
 */
export function permissionTargets(view: PermissionView): readonly PermissionTarget[] {
  return Object.freeze(
    view.permissions.map((permission) => {
      const target = Object.freeze({
        conversationId: view.conversationId,
        review: permission.review,
        executionId: permission.executionId,
        permissionId: permission.permissionId,
      }) as PermissionTarget
      offered.set(target, new Set(permission.options.map((option) => option.id)))
      return target
    }),
  )
}

/** Read an issued target's conversation before routing local control state. */
export function permissionTargetConversation(target: PermissionTarget): string {
  return permissionFields(target).conversationId
}

/** @internal Serialize only a factory-issued target and its published choice. */
export function permissionFields(target: PermissionTarget, optionId?: string) {
  const options = offered.get(target)
  if (!options) throw new TypeError("Use a permission target captured from a view")
  if (optionId !== undefined && !options.has(optionId))
    throw new TypeError("Select a choice offered by the captured review")
  const { conversationId, review, executionId, permissionId } = target
  return { conversationId, review, executionId, permissionId }
}
