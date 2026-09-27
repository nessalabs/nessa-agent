import * as React from "react"
import { isImageFile, type useConversation } from "../../conversation"
import type { AttachmentResources } from "../adapters/attachment-resources"
import { nextUploads, uploadImage } from "../application/upload-image"

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
  const inFlight = React.useRef(new Map<string, AbortController>())
  const settled = React.useRef(new Set<string>())
  const latest = React.useRef({ chat, resources, digest })
  const pump = React.useCallback(() => {
    const { chat, resources, digest } = latest.current
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
            isImageFile(part.mimeType) &&
            !settled.current.has(part.id)
              ? [
                  {
                    conversationId: conversation.id,
                    id: part.id,
                    mimeType: part.mimeType,
                  },
                ]
              : [],
          ),
    )
    for (const file of nextUploads(waiting, new Set(inFlight.current.keys()))) {
      const stopping = new AbortController()
      inFlight.current.set(file.id, stopping)
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
        if (inFlight.current.get(file.id) === stopping) inFlight.current.delete(file.id)
        settled.current.add(file.id)
        pump()
      })
    }
  }, [])
  React.useEffect(() => {
    latest.current = { chat, resources, digest }
    // A render after an upload settled has that upload's outcome in it.
    settled.current.clear()
    const drafted = new Set(
      chat.conversations.flatMap((conversation) =>
        conversation.draft.flatMap((part) => (part.type === "file" ? [part.id] : [])),
      ),
    )
    for (const [fileId, stopping] of inFlight.current) {
      if (drafted.has(fileId)) continue
      stopping.abort()
      inFlight.current.delete(fileId)
    }
    pump()
  }, [chat, resources, digest, pump])
  return {
    /** Try a failed upload again. The effect above picks it up from there. */
    retry: (fileId: string) => chat.changeUpload({ fileId, to: "not-started" }),
  }
}
