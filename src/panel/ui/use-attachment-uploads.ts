import * as React from "react"
import { isImageFile, type useConversation } from "../../conversation"
import type { AttachmentResources } from "../adapters/attachment-resources"
import { sha256Digest } from "../adapters/sha256"
import { uploadImage } from "../application/upload-image"

/**
 * Starts an upload for every draft image that has not had one, in any tab.
 *
 * Driven by state rather than by the attach call, so there is one way an upload
 * begins: an image sitting at `not-started`. Attaching produces that, and so
 * does asking to retry a failed one — which is all `retry` does. Any `image/*`
 * file qualifies; whether the gateway can read it is the gateway's answer. The
 * order of the work is `uploadImage`'s; this only decides when to call it.
 *
 * `started` keeps a file from being picked up twice while its first state
 * change is still on its way back through React — Strict Mode runs this effect
 * twice on mount against the same snapshot. It lives in a ref because it is
 * bookkeeping about calls made, not anything to paint.
 */
export function useAttachmentUploads(
  chat: ReturnType<typeof useConversation>,
  resources: AttachmentResources,
  digest: (bytes: Blob) => Promise<string> = sha256Digest,
) {
  const started = React.useRef(new Set<string>())
  const upload = React.useEffectEvent(
    (file: Parameters<typeof uploadImage>[0]): Promise<void> =>
      uploadImage(file, {
        digest,
        bytes: resources.bytes,
        change: chat.changeUpload,
        stage: chat.stageAttachment,
      }),
  )
  React.useEffect(() => {
    for (const conversation of chat.conversations) {
      for (const part of conversation.draft) {
        if (
          part.type !== "file" ||
          part.upload.status !== "not-started" ||
          !isImageFile(part.mimeType) ||
          started.current.has(part.id)
        )
          continue
        const { id, mimeType } = part
        started.current.add(id)
        void upload({ conversationId: conversation.id, id, mimeType }).finally(() =>
          started.current.delete(id),
        )
      }
    }
  }, [chat.conversations])
  return {
    /** Try a failed upload again. The effect above picks it up from there. */
    retry: (fileId: string) => chat.changeUpload({ fileId, to: "not-started" }),
  }
}
