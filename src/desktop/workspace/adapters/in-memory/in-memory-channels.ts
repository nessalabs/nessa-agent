/**
 * A `ChannelSource` held in memory, for the sample workspace and tests: it
 * makes a channel from the draft, lists it through `add` (the sample
 * source's index, which places it in a section), and answers the same channel for the same request
 * id, as the gateway does. A refusal is one a test asks for (`refuse`), as a
 * typed `ChannelCreationError`.
 */
import { ChannelCreationError, type ChannelSource } from "../../application/ports"
import {
  channelName,
  type ChannelDraft,
  type ChannelFailure,
} from "../../model/channel-draft"
import type { Channel } from "../../model/workspace-index"

export interface InMemoryChannels extends ChannelSource {
  /** Each Create asked, in order, with its request id and draft. */
  readonly calls: readonly { readonly requestId: string; readonly draft: ChannelDraft }[]
  /** The next Create is refused with `failure`. */
  refuse(failure: ChannelFailure): void
}

export function inMemoryChannels({
  add,
}: {
  add: (channel: Omit<Channel, "sectionId">) => Channel
}): InMemoryChannels {
  const calls: { requestId: string; draft: ChannelDraft }[] = []
  const made = new Map<string, Channel>()
  let refusal: ChannelFailure | null = null
  return {
    calls,
    refuse(failure) {
      refusal = failure
    },
    create(draft, requestId) {
      calls.push({ requestId, draft })
      const held = made.get(requestId)
      if (held) return Promise.resolve(held)
      const name = channelName(draft.name)
      if (refusal || name === null) {
        const failure = refusal ?? "invalid"
        refusal = null
        return Promise.reject(new ChannelCreationError(failure))
      }
      const id = `sample-channel-${made.size + 1}`
      const channel = add({
        id,
        name,
        private: false,
        topic: "",
        folder: draft.folder ?? `~/.nessa/channels/${id}`,
        defaults: { model: draft.model, access: draft.access },
        coordinatorPlacement: draft.coordinatorPlacement,
        threadPlacement: draft.threadPlacement,
        coordinatorConversationId: `sample-coordinator-${made.size + 1}`,
      })
      made.set(requestId, channel)
      return Promise.resolve(channel)
    },
  }
}
