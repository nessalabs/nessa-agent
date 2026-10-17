import {
  permissionTargets,
  type ConversationView as GatewayConversationView,
  type PermissionTarget,
} from "@nessa/client"

const utf8 = new TextEncoder()

/**
 * The whole inputs of a conversation's waiting reviews, as this panel has
 * read them.
 *
 * A view carries a long input as its start only (`argumentsComplete` false).
 * Each such review's whole input is read once with `permissionInput`, and
 * every view after that carries it whole. Until it is read the review stays
 * incomplete, and no choice is offered on it (`conversation-controls.tsx`).
 * A read that fails is asked again with the next view that names the review.
 */
export function reviewInputs(read: (target: PermissionTarget) => Promise<string>) {
  const held = new Map<string, string>()
  const asking = new Set<string>()
  const keyOf = (review: { executionId: string; permissionId: string }) =>
    JSON.stringify([review.executionId, review.permissionId])
  return {
    /** `view`, with each review whose whole input is held carrying all of it. */
    complete(view: GatewayConversationView): GatewayConversationView {
      if (view.permissions.every((review) => review.argumentsComplete)) return view
      return {
        ...view,
        permissions: view.permissions.map((review) => {
          const whole = review.argumentsComplete ? undefined : held.get(keyOf(review))
          return whole === undefined
            ? review
            : { ...review, argumentsJson: whole, argumentsComplete: true }
        }),
      }
    },
    /**
     * Reads the whole input of each review in `view` that is not held, and
     * calls `loaded` once one arrives. Inputs of reviews `view` no longer
     * names are let go.
     */
    read(view: GatewayConversationView, loaded: () => void) {
      const named = new Set(view.permissions.map(keyOf))
      for (const key of held.keys()) if (!named.has(key)) held.delete(key)
      const targets = permissionTargets(view)
      view.permissions.forEach((review, index) => {
        const key = keyOf(review)
        const target = targets[index]
        if (review.argumentsComplete || held.has(key) || asking.has(key) || !target)
          return
        asking.add(key)
        read(target).then(
          (whole) => {
            asking.delete(key)
            // The client checked every range; this checks it is this review's.
            if (
              utf8.encode(whole).byteLength !== review.argumentsBytes ||
              !whole.startsWith(review.argumentsJson)
            )
              return
            held.set(key, whole)
            loaded()
          },
          () => asking.delete(key),
        )
      })
    },
  }
}
