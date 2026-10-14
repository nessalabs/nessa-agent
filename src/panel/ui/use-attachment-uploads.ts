import * as React from "react"
import { isImageFile, type useConversation } from "../../conversation"
import type { AttachmentResources } from "../adapters/attachment-resources"
import { nextUploads, uploadImage } from "../application/upload-image"

type UploadQueue = {
  inFlight: Map<string, AbortController>
  settled: Set<string>
  pump?: () => void
}
// One resource manager belongs to one store lifetime. Weak ownership keeps
// placement numbers local without sharing slots when a Provider changes stores.
const queues = new WeakMap<AttachmentResources, UploadQueue>()
function queueOf(resources: AttachmentResources): UploadQueue {
  let queue = queues.get(resources)
  if (!queue) {
    queue = { inFlight: new Map(), settled: new Set() }
    queues.set(resources, queue)
  }
  return queue
}

/**
 * Starts uploads for draft images that have not had one, in any tab, a few at a
 * time.
 *
 * Driven by state rather than by the attach call, so there is one way an upload
 * begins: an image sitting at `not-started`. Attaching produces that, and so
 * does asking to retry a failed one — which is all `retry` does. Any `image/*`
 * file qualifies; whether the gateway can read it is the gateway's answer. The
 * order of one upload is `uploadImage`'s and which ones go next is
 * `nextUploads`'s; this only decides when to ask.
 *
 * It asks twice: when the drafts change, and when an upload finishes. The second
 * matters. An image waiting for a slot is already `not-started`, so nothing about
 * the drafts changes when a slot frees, and an effect alone would leave it
 * waiting for good.
 *
 * `inFlight` holds, for each upload under way, the means to stop it. It keeps a
 * file from being picked up twice while its first state change is still on its
 * way back through React — Strict Mode runs the effect twice on mount against
 * the same snapshot. And when a tile is taken away its upload is stopped and its
 * slot given back at once: left to finish, a removed 64 MiB file would hold one
 * of this window's three slots, and one of the gateway's four, for minutes.
 *
 * `settled` is the same care at the other end. The ask that follows a finished
 * upload reads the drafts as React last rendered them, and an upload can finish
 * before React has rendered at all — its bytes were already gone — leaving that
 * file looking `not-started` in a snapshot that is simply old. Starting it again
 * from there would finish the same way, at once, for ever. So a file that has
 * settled is not chosen again until a newer snapshot says it should be. The
 * refs are bookkeeping about calls made and what was last seen, not anything to
 * paint.
 */
export function useAttachmentUploads(
  chat: ReturnType<typeof useConversation>,
  resources: AttachmentResources,
  digest: (bytes: Blob) => Promise<string>,
) {
  const queue = queueOf(resources)
  const latest = React.useRef({ chat, resources, digest, queue })
  const pump = React.useCallback(() => {
    const { chat, resources, digest, queue } = latest.current
    const waiting = chat.conversations.flatMap((conversation) =>
      // An agent that has said it takes no images is not asked to take one.
      // Attaching refuses an image for such a conversation outright, so the
      // case left here is the one attaching cannot catch: a file attached
      // while the answer was still unknown, with the answer arriving after.
      // Spending the upload anyway costs the window's bytes and the gateway's
      // slots for a refusal that is already certain, and lands the tile in the
      // red that means something went wrong. Nothing did.
      //
      // `undefined` still uploads. It is not a no, and an image refused on a
      // gateway that had simply not answered yet would be refused for good.
      conversation.remote?.capabilities.imageInput === false
        ? []
        : conversation.draft.flatMap((part) =>
            part.type === "file" &&
            part.upload.status === "not-started" &&
            isImageFile(part.resource.mimeType) &&
            !queue.settled.has(part.placementId)
              ? [
                  {
                    conversationId: conversation.id,
                    placementId: part.placementId,
                    resource: part.resource,
                  },
                ]
              : [],
          ),
    )
    for (const file of nextUploads(waiting, new Set(queue.inFlight.keys()))) {
      const stopping = new AbortController()
      queue.inFlight.set(file.placementId, stopping)
      void uploadImage(
        file,
        {
          digest,
          bytes: resources.bytes,
          change: chat.changeUpload,
          stage: chat.stageAttachment,
        },
        stopping.signal,
      ).finally(() => {
        // Only this upload's own entry: a removed file's slot was given back
        // already, and may since have gone to another file.
        if (queue.inFlight.get(file.placementId) === stopping)
          queue.inFlight.delete(file.placementId)
        queue.settled.add(file.placementId)
        queue.pump?.()
      })
    }
  }, [])
  React.useEffect(() => {
    latest.current = { chat, resources, digest, queue }
    // A render after an upload settled has that upload's outcome in it.
    queue.settled.clear()
    const drafted = new Set(
      chat.conversations.flatMap((conversation) =>
        conversation.draft.flatMap((part) =>
          part.type === "file" ? [part.placementId] : [],
        ),
      ),
    )
    for (const [placementId, stopping] of queue.inFlight) {
      if (drafted.has(placementId)) continue
      stopping.abort()
      queue.inFlight.delete(placementId)
    }
    queue.pump = pump
    pump()
    return () => {
      if (queue.pump === pump) delete queue.pump
    }
  }, [chat, resources, digest, queue, pump])
  return {
    /** Try a failed upload again. The effect above picks it up from there. */
    retry: (placementId: string) =>
      chat.changeUpload({
        conversationId: chat.active.id,
        placementId,
        to: "not-started",
      }),
  }
}
