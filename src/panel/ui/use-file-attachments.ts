import * as React from "react"
import {
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  type FileAttachment,
  type useConversation,
} from "../../conversation"
import { readDroppedImage } from "../adapters/dropped-image"
import { readAttachment } from "../adapters/read-attachment"

/** Coordinates local file selection and preview against the originating conversation. */
export function useFileAttachments(chat: ReturnType<typeof useConversation>) {
  const conversationsRef = React.useRef(chat.conversations)
  React.useLayoutEffect(() => {
    conversationsRef.current = chat.conversations
  }, [chat.conversations])
  const inputRef = React.useRef<HTMLInputElement>(null)
  const [pending, setPending] = React.useState<{
    conversationId: string
    files: { name: string; mimeType: string; previewUrl?: string }[]
  } | null>(null)
  const pendingUrls = React.useRef<string[]>([])
  const download = React.useRef<AbortController | null>(null)
  const busyRef = React.useRef(false)
  const pendingConversation = React.useRef<string | null>(null)
  const mounted = React.useRef(true)
  const [reading, setReading] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const [viewed, setViewed] = React.useState<{
    conversationId: string
    file: FileAttachment
  } | null>(null)
  React.useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      download.current?.abort()
      pendingUrls.current.forEach((url) => URL.revokeObjectURL(url))
      pendingUrls.current = []
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
  const files = chat.active.draft.filter(
    (part): part is FileAttachment => part.type === "file",
  )
  async function addFiles(selected: readonly File[], targetId = chat.active.id) {
    if (!selected.length) return
    if (busyRef.current) {
      setError("Please wait for the selected files to finish loading.")
      return
    }
    const conversationId = targetId
    const target = conversationsRef.current.find(
      (conversation) => conversation.id === targetId,
    )
    if (!target) return
    const targetFiles = target.draft.filter((part) => part.type === "file")
    if (
      selected.some((file) => file.size > MAX_ATTACHMENT_BYTES) ||
      targetFiles.length + selected.length > MAX_DRAFT_ATTACHMENTS ||
      [...targetFiles, ...selected].reduce((total, file) => total + file.size, 0) >
        MAX_DRAFT_ATTACHMENT_BYTES
    ) {
      setError("Attach up to 20 files, 20 MB each and 50 MB total per draft.")
      return
    }
    busyRef.current = true
    pendingConversation.current = conversationId
    setPending({
      conversationId,
      files: selected.map((file) => {
        const previewUrl = file.type.startsWith("image/")
          ? URL.createObjectURL(file)
          : undefined
        if (previewUrl) pendingUrls.current.push(previewUrl)
        return { name: file.name, mimeType: file.type, previewUrl }
      }),
    })
    setReading(true)
    setError(null)
    try {
      const attachments: FileAttachment[] = []
      for (const file of selected) attachments.push(await readAttachment(file))
      if (mounted.current) chat.attachFiles(attachments, conversationId)
    } catch {
      if (mounted.current)
        setError("One of the files could not be read. Please select the files again.")
    } finally {
      busyRef.current = false
      pendingConversation.current = null
      pendingUrls.current.forEach((url) => URL.revokeObjectURL(url))
      pendingUrls.current = []
      if (mounted.current) {
        setReading(false)
        setPending(null)
      }
    }
  }
  async function addImageUrl(url: string) {
    if (busyRef.current) return
    const targetId = chat.active.id
    busyRef.current = true
    pendingConversation.current = targetId
    setReading(true)
    setError(null)
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
      await addFiles([file], targetId)
    } catch {
      if (mounted.current)
        setError(
          "The image could not be loaded. Save the image, then drop the file here.",
        )
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
    error,
    setError,
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
