/**
 * A conversation view with every review it lists read in full.
 *
 * A view lists each waiting review by its key alone; what it shows, and the
 * choices an answer may name, are read from its live owner with
 * `conversation.permissionReview`. A review answered or withdrawn between
 * the two reads is left out, as the next view would leave it out.
 */
export async function withReviews(client, view) {
  const permissions = []
  for (const entry of view.permissions) {
    try {
      permissions.push(
        await client.conversation.permissionReview(view.conversationId, entry.review),
      )
    } catch (error) {
      if (error?.code !== "stale_permission") throw error
    }
  }
  return { ...view, permissions }
}

/** Read `conversationId` with every review it lists read in full. */
export async function readWithReviews(client, conversationId) {
  return withReviews(client, await client.conversation.read(conversationId))
}
