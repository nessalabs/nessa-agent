import type { WorkspaceState } from "../workspace-state"

/** Every image URL the workspace names: in a composer, or on a message in the outbox. */
function namedImageUrls(state: WorkspaceState): Set<string> {
  const urls = new Set<string>()
  for (const images of Object.values(state.composerImages))
    for (const image of images) urls.add(image.url)
  for (const messages of Object.values(state.outbox))
    for (const message of messages)
      for (const image of message.localImages ?? []) urls.add(image.url)
  return urls
}

/**
 * The image URLs `previous` named and `current` does not: removed from a
 * composer, its session let go, or its message now held by the conversation
 * or discarded. The state is the one authority on whether a pasted image's
 * bytes are still wanted, so this is what lets them go.
 */
export function releasedImageUrls(
  previous: WorkspaceState,
  current: WorkspaceState,
): string[] {
  if (
    previous.composerImages === current.composerImages &&
    previous.outbox === current.outbox
  )
    return []
  const kept = namedImageUrls(current)
  return [...namedImageUrls(previous)].filter((url) => !kept.has(url))
}
