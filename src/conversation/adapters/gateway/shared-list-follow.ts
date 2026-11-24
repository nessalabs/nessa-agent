import type { ConversationListFollower } from "../../application/ports"
import type { ConversationListing } from "../../application/view"

type OpenList = (archived: boolean, follower: ConversationListFollower) => () => void

/**
 * One gateway list follow for each of archived and not, however many
 * followers want it.
 *
 * The gateway keeps one live subscription per target on a connection and
 * refuses a second as a duplicate, so two followers of the same list (the
 * panel's Messages and its asked decisions) must share one. The first
 * follower opens it, each list goes to every follower, a follower that joins
 * later is told the last list at once, and the last one to stop closes it.
 */
export function sharedListFollows(open: OpenList): OpenList {
  const shared = new Map<
    boolean,
    {
      followers: Set<{ follower: ConversationListFollower }>
      last: ConversationListing | undefined
      stop: () => void
    }
  >()
  return (archived, follower) => {
    let entry = shared.get(archived)
    const mine = { follower }
    if (!entry) {
      const created = {
        followers: new Set([mine]),
        last: undefined as ConversationListing | undefined,
        stop: () => {},
      }
      shared.set(archived, created)
      entry = created
      created.stop = open(archived, {
        list(list) {
          created.last = list
          for (const each of [...created.followers]) each.follower.list(list)
        },
        failed(reason, cause) {
          for (const each of [...created.followers]) each.follower.failed(reason, cause)
        },
      })
    } else {
      const joined = entry
      joined.followers.add(mine)
      if (joined.last) {
        const last = joined.last
        queueMicrotask(() => {
          if (joined.followers.has(mine)) follower.list(last)
        })
      }
    }
    const current = entry
    return () => {
      if (!current.followers.delete(mine)) return
      if (current.followers.size > 0) return
      if (shared.get(archived) === current) shared.delete(archived)
      current.stop()
    }
  }
}
