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
   * Why the last thing offered was not taken, and what it was said about.
   *
   * Typed rather than a sentence: the words, and whether there is anything to
   * be done, belong to `attachment-notice`. Kept with the conversation it was
   * refused in and with that draft's files as they stood, because a refusal
   * describes a situation and stops being true when the situation does. "Attach
   * fewer, or send what is here first" is not something to go on saying above a
   * draft somebody has since emptied, and it was never about the conversation
   * in the next tab.
   */
  const [refused, setRefused] = React.useState<{
    conversationId: string
    draftFiles: string
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
  /** A draft's files as one comparable value: which files, in which order. */
  const draftFilesOf = (conversationId: string) =>
    (conversationsRef.current.find((item) => item.id === conversationId)?.draft ?? [])
      .flatMap((part) => (part.type === "file" ? [part.id] : []))
      .join(" ")
  /**
   * Remember a refusal against the draft it is about. Defaults to the active
   * conversation; a folder walk that finishes after somebody has moved on names
   * the one it was dropped into.
   */
  const refuse = (refusal: AttachmentRefusal, conversationId = chat.active.id) =>
    setRefused({
      conversationId,
      draftFiles: draftFilesOf(conversationId),
      refusal,
    })
  // Shown only where and while it is still true: this conversation, and this
  // draft. Attaching, removing a tile, and sending all change the draft, and
  // each of them answers the refusal by doing what it asked or overtaking it.
  const refusal =
    refused?.conversationId === chat.active.id &&
    refused.draftFiles === files.map((file) => file.id).join(" ")
      ? refused.refusal
      : null
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
    setRefused(null)
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
    setRefused(null)
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
    clearRefusal: () => setRefused(null),
    addFiles,
    addImageUrl,
    chooseFiles: () => inputRef.current?.click(),
    viewed: viewed?.conversationId === chat.active.id ? viewed.file : null,
    open: (file: FileAttachment) => setViewed({ conversationId: chat.active.id, file }),
    close: () => setViewed(null),
    remove: (id: string) => {
      chat.removeFile(id)
      if (viewed?.file.id === id) setViewed(null)
    },
  }
}
