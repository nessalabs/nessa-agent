/**
 * A session's composer attachments, wired to the workspace
 * (`composerImages`): the tiles show what the store holds, a paste or a pick
 * asks `addComposerImages`, and one refused is said until they change.
 */
import { useCallback, useMemo, useState } from "react"
import {
  addComposerImages,
  DRAFT_IMAGE_LIMITS,
  removeComposerImage,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { useRememberedValue } from "../../adapters/store/remembered-session"
import { selectComposerImages } from "../../adapters/store/selectors"
import type { DraftImageRefusal } from "../../model/draft-images"
import type { ComposerImages } from "../../../ui/composer"

const mebibytes = (bytes: number) => `${bytes / (1024 * 1024)} MiB`

/** What the composer says of attachments it did not take. */
export function draftImageRefusalCopy(refusal: DraftImageRefusal): string {
  switch (refusal) {
    case "too-many":
      return `A message can carry ${DRAFT_IMAGE_LIMITS.images} images at most.`
    case "too-large":
      return `An image can be ${mebibytes(DRAFT_IMAGE_LIMITS.bytes)} at most.`
    case "too-heavy":
      return `A message’s images can be ${mebibytes(DRAFT_IMAGE_LIMITS.totalBytes)} at most in all.`
    case "too-many-files":
      return `A message can carry ${DRAFT_IMAGE_LIMITS.files} files at most.`
    case "too-heavy-files":
      return `A message’s files can be ${mebibytes(DRAFT_IMAGE_LIMITS.fileBytes)} at most in all.`
    case "empty":
      return "An empty file can’t be sent."
    case "unsupported":
      return `Images can be ${DRAFT_IMAGE_LIMITS.types
        .map((type) => type.replace(/^image\//, "").toUpperCase())
        .join(", ")}.`
  }
}

/** The attachments on a session's next message, and what a paste, a pick and a remove do. */
export function useComposerImages(sessionId: string): ComposerImages {
  const dispatch = useWorkspaceDispatch()
  const live = useWorkspaceSelector((state) => selectComposerImages(state, sessionId))
  const images = useRememberedValue(sessionId, live)
  // The refusal answers one paste, for the images held when it was said.
  const [refused, setRefused] = useState<{
    readonly images: typeof images
    readonly refusal: DraftImageRefusal
  } | null>(null)
  const onPaste = useCallback(
    (files: File[]) => {
      const refusal = dispatch(addComposerImages({ sessionId, files }))
      setRefused(refusal ? { images: live, refusal } : null)
    },
    [dispatch, live, sessionId],
  )
  const onRemove = useCallback(
    (imageId: string) => dispatch(removeComposerImage({ sessionId, imageId })),
    [dispatch, sessionId],
  )
  const notice =
    refused && refused.images === images
      ? draftImageRefusalCopy(refused.refusal)
      : undefined
  return useMemo(
    () => ({ images, onPaste, onRemove, notice }),
    [images, onPaste, onRemove, notice],
  )
}
