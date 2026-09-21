import * as React from "react"
import {
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  type FileAttachment,
  type useConversation,
} from "../../conversation"
import { readDroppedImage } from "../adapters/dropped-image"
import {
  MAX_SESSION_ATTACHMENT_BYTES,
  type AttachmentResources,
} from "../adapters/attachment-resources"
import type { AttachmentRefusal } from "../application/attachment-notice"

const MIB = 1024 * 1024

/** Coordinates local file selection and preview against the originating conversation. */
/**
 * Fetch the preview's own chunk before anybody asks for it.
 *
 * Opening a file is one click with nothing before it, and the preview arrives
 * with the design system's whole set of renderers behind it — Markdown, JSON,
 * PDF, video — because the one that draws a picture is registered beside them.
 * Waiting for all that after the click is a sheet that says "Loading preview…"
 * for as long as the fetch takes.
 *
 * Attaching a file is the moment somebody might open one, and it is a moment
 * with nothing else going on, so the chunk is fetched then and the click has
 * nothing left to wait for. Repeat calls cost nothing: the same specifier
 * resolves to the module already in hand, which is also what lets this and the
 * panel's `React.lazy` of it agree.
 */
const warmPreview = () => void import("./attachment-preview")

export function useFileAttachments(
  chat: ReturnType<typeof useConversation>,
  resources: AttachmentResources,
) {
  const conversationsRef = React.useRef(chat.conversations)
  React.useLayoutEffect(() => {
    conversationsRef.current = chat.conversations
  }, [chat.conversations])
  const inputRef = React.useRef<HTMLInputElement>(null)
  const [pending, setPending] = React.useState<{
    conversationId: string
    files: { name: string; mimeType: string; previewUrl?: string }[]
  } | null>(null)
  const download = React.useRef<AbortController | null>(null)
  const busyRef = React.useRef(false)
  const pendingConversation = React.useRef<string | null>(null)
  const mounted = React.useRef(true)
  const [reading, setReading] = React.useState(false)
  /**
   * Why the last thing offered was not taken, and which conversation it was
   * said to.
   *
   * Typed rather than a sentence: the words, and whether there is anything to
   * be done, belong to `attachment-notice`.
   *
   * A refusal answers one attempt and lives until something answers it back —
   * and what those things are is written down here as calls to
   * {@link answerRefusal}, not inferred from the draft afterwards. Inferring
   * was tried and is wrong in both directions: a comparison taken while the
   * handler runs is older than the attach happening in the same handler, so a
   * drop that attached one file and refused another said nothing at all; and a
   * draft restored after a refused send brings its files back under the same
   * identities, so a refusal that had been answered came back with them.
   */
  const [refused, setRefused] = React.useState<{
    conversationId: string
    refusal: AttachmentRefusal
  } | null>(null)
  const [viewed, setViewed] = React.useState<{
    conversationId: string
    file: FileAttachment
  } | null>(null)
  React.useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      download.current?.abort()
    }
  }, [])
  React.useEffect(() => {
    if (
      viewed &&
      !chat.conversations.some(
        (conversation) =>
          conversation.id === viewed.conversationId &&
          conversation.draft.some(
            (part) => part.type === "file" && part.id === viewed.file.id,
          ),
      )
    )
      setViewed(null)
  }, [chat.conversations, viewed])
  // Somebody with a file attached is somebody who may open it.
  const anyFiles = chat.conversations.some((conversation) =>
    conversation.draft.some((part) => part.type === "file"),
  )
  React.useEffect(() => {
    if (anyFiles) warmPreview()
  }, [anyFiles])
  const files = chat.active.draft.filter(
    (part): part is FileAttachment => part.type === "file",
  )
  /**
   * Say why something was not taken. Defaults to the active conversation; a
   * folder walk that finishes after somebody has moved on names the one it was
   * dropped into, so its refusal is not shown above another draft quoting
   * another draft's bounds.
   *
   * Replaces whatever was there. There is one of these at a time, and it is
   * always the most recent thing the panel turned away.
   */
  const refuse = (refusal: AttachmentRefusal, conversationId = chat.active.id) =>
    setRefused({ conversationId, refusal })
  /**
   * Put down a refusal this conversation has now answered, leaving another
   * tab's alone. The answers are: files actually attached, a file taken off the
   * draft, and the draft going to the gateway. Each of them is the person
   * having moved on from the attempt the refusal was about.
   *
   * Written as an update rather than a read, so it is correct from inside an
   * event handler that is also attaching — which is where the ordering matters:
   * the drop zone hands over what passed before what it refused, and the
   * refusal must be what survives the pair.
   */
  const answerRefusal = (conversationId: string) =>
    setRefused((current) => (current?.conversationId === conversationId ? null : current))
  /** Said only to the conversation it was said about. */
  const refusal = refused?.conversationId === chat.active.id ? refused.refusal : null
  function addFiles(selected: readonly File[], targetId = chat.active.id) {
    if (!selected.length) return
    if (busyRef.current) {
      refuse({ reason: "reading-files" }, targetId)
      return
    }
    const conversationId = targetId
    const target = conversationsRef.current.find(
      (conversation) => conversation.id === targetId,
    )
    if (!target) return
    const targetFiles = target.draft.filter((part) => part.type === "file")
    // Three bounds, told apart, because the advice for each is different and
    // one of them has no advice at all: a file over the per-file bound is
    // refused by every route into this composer.
    const tooLarge = selected.filter((file) => file.size > MAX_ATTACHMENT_BYTES)
    if (tooLarge.length > 0) {
      refuse(
        { reason: "file-too-large", names: tooLarge.map((file) => file.name) },
        targetId,
      )
      return
    }
    if (targetFiles.length + selected.length > MAX_DRAFT_ATTACHMENTS) {
      refuse({ reason: "too-many-files" }, targetId)
      return
    }
    if (
      [...targetFiles, ...selected].reduce((total, file) => total + file.size, 0) >
      MAX_DRAFT_ATTACHMENT_BYTES
    ) {
      refuse({ reason: "draft-too-large" }, targetId)
      return
    }
    if (!resources.canAdd(selected.reduce((total, file) => total + file.size, 0))) {
      refuse(
        {
          reason: "window-budget",
          maxMiB: MAX_SESSION_ATTACHMENT_BYTES / MIB,
          draftsHoldFiles: conversationsRef.current.some((conversation) =>
            conversation.draft.some((part) => part.type === "file"),
          ),
        },
        targetId,
      )
      return
    }
    answerRefusal(targetId)
    try {
      // Object URLs are ready synchronously; no FileReader or duplicate thumbnail URLs.
      chat.attachFiles(resources.add(selected), conversationId)
    } catch {
      refuse({ reason: "unreadable-files" }, targetId)
    }
  }

  async function addImageUrl(url: string) {
    if (busyRef.current) return
    const targetId = chat.active.id
    busyRef.current = true
    pendingConversation.current = targetId
    setReading(true)
    answerRefusal(targetId)
    setPending({
      conversationId: targetId,
      files: [{ name: "Image", mimeType: "image/" }],
    })
    const controller = new AbortController()
    download.current = controller
    try {
      const file = await readDroppedImage(url, controller.signal, MAX_ATTACHMENT_BYTES)
      if (!mounted.current) return
      busyRef.current = false
      setReading(false)
      setPending(null)
      addFiles([file], targetId)
    } catch {
      if (mounted.current) refuse({ reason: "unreadable-image-url" }, targetId)
    } finally {
      download.current = null
      pendingConversation.current = null
      busyRef.current = false
      if (mounted.current) {
        setReading(false)
        setPending(null)
      }
    }
  }
  return {
    inputRef,
    isPending: (conversationId: string) => pendingConversation.current === conversationId,
    pendingFiles: pending?.conversationId === chat.active.id ? pending.files : [],
    files,
    reading,
    refusal,
    refuse,
    /**
     * The draft has gone to the gateway, so whatever was refused before it is
     * over. Called for a submission that was actually taken — a send the panel
     * turned away has not moved anybody on, and is usually itself the refusal
     * being shown.
     */
    draftSent: answerRefusal,
    clearRefusal: () => setRefused(null),
    addFiles,
    addImageUrl,
    chooseFiles: () => inputRef.current?.click(),
    viewed: viewed?.conversationId === chat.active.id ? viewed.file : null,
    open: (file: FileAttachment) => setViewed({ conversationId: chat.active.id, file }),
    close: () => setViewed(null),
    remove: (id: string) => {
      chat.removeFile(id)
      // Taking a tile off is an answer to "attach fewer" and to "this draft is
      // too heavy" alike, whichever of them was being said.
      answerRefusal(chat.active.id)
      if (viewed?.file.id === id) setViewed(null)
    },
  }
}
