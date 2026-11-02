import assert from "node:assert/strict"

/** Fixture approvals reserve the single delegated child before answering.
 * A failed answer may already have executed; it never restores that allowance. */
export function fixtureApprovals(permission) {
  const approved = new Set()
  let childReserved = false
  return {
    get size() {
      return approved.size
    },
    has(reviewId) {
      return approved.has(reviewId)
    },
    async answer(review, answer) {
      if (approved.has(review.review)) return
      const delegation =
        permission === "delegate" && review.toolName.includes("nessa-agents")
      const create = delegation && review.toolName.endsWith("create_thread")
      assert(
        (permission === "log" && review.toolName.includes("read_local_log")) ||
          (delegation && (create || review.toolName.endsWith("send_message"))),
        "Unexpected approval; no broad permission is granted",
      )
      assert(!create || !childReserved, "Only one delegated child is allowed")
      if (create) childReserved = true
      approved.add(review.review)
      await answer()
    },
  }
}
