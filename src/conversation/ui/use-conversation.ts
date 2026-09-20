import { pollConversation } from "../adapters/gateway/polling"
import { useEffect, useEffectEvent, useState } from "react"
import { type FileAttachment, type MessageContent } from "../model"
import type { UploadChange, UploadedFile } from "../application/ports"
import { activeConversation } from "../application/queries/active-conversation"
import {
  renameConversation,
  attachFiles,
  removeFile,
  uploadChanged,
  stageAttachment,
  closeTab,
  openConversation,
  sendDraft,
  setActive,
  moveActive,
  setDraft,
  stopGenerating,
  refreshConversation,
  invalidateRead,
} from "../adapters/store/slice"
import { useConversationDispatch, useConversationSelector } from "../adapters/store/hooks"
import { canUseGateway } from "../../session"

export function useConversation() {
  const [deliveryMode, setDeliveryMode] = useState<"queue" | "steer">("queue")
  const dispatch = useConversationDispatch()
  const tabs = useConversationSelector((state) => state.conversation)
  const gatewayAvailable = useConversationSelector((state) =>
    canUseGateway(state.session),
  )
  const conversations = tabs.conversations
  const active = activeConversation(tabs)
  const pollingDelay = useEffectEvent(() =>
    active.phase === "idle" && !active.remote?.permissions.length ? 2000 : 250,
  )

  useEffect(() => {
    if (!active.serverReady || !gatewayAvailable) return
    return pollConversation(
      () => dispatch(refreshConversation(active.id)),
      () => {
        dispatch(invalidateRead(active.id))
      },
      pollingDelay,
    )
  }, [dispatch, active.id, active.serverReady, gatewayAvailable])

  return {
    rename: (id: string, title: string) => dispatch(renameConversation({ id, title })),
    deliveryMode,
    setDeliveryMode,
    attachFiles: (files: FileAttachment[], conversationId: string) =>
      dispatch(attachFiles({ files, conversationId })),
    removeFile: (id: string) => dispatch(removeFile(id)),
    /** Ask for one step in a draft file's upload. Steps that do not follow are ignored. */
    changeUpload: (change: UploadChange) => dispatch(uploadChanged(change)),
    /**
     * Upload a draft image's original bytes. Settles when the file's upload
     * state says how it went; it never rejects.
     */
    stageAttachment: async (input: {
      conversationId: string
      fileId: string
      file: UploadedFile
      bytes: Blob
    }): Promise<void> => {
      await dispatch(
        stageAttachment({
          id: input.conversationId,
          fileId: input.fileId,
          file: input.file,
          bytes: input.bytes,
        }),
      )
    },
    conversations,
    active,
    gatewayAvailable,
    moveActive: (direction: -1 | 1) => dispatch(moveActive(direction)),
    setActive: (id: string) => dispatch(setActive(id)),
    /**
     * Sends the draft, and says whether it was taken.
     *
     * Taken means the draft left the composer — the submission started and the
     * transcript now owns the message. It is not a claim about delivery: a
     * gateway that fails afterwards leaves a message that was sent and did not
     * arrive, which is the transcript's to show, not the composer's.
     *
     * Every local refusal rejects with a reason, and only a failure after
     * admission throws, so the two are told apart by whether a payload came
     * back rather than by the store being read a second time.
     */
    submit: async (content: MessageContent): Promise<boolean> => {
      const finished = await dispatch(
        sendDraft({
          content,
          id: active.id,
          // Not an early return here: `sendDraft` declines it with a reason.
          connected: gatewayAvailable,
          steering: deliveryMode === "steer" && active.phase !== "idle",
        }),
      )
      if (!sendDraft.rejected.match(finished)) return true
      return finished.payload === undefined
    },
    openConversation: () => {
      dispatch(openConversation())
    },
    closeConversation: (id: string) => {
      void dispatch(closeTab(id))
    },
    setDraft: (draft: MessageContent) => dispatch(setDraft({ draft })),
    stopGenerating: () => dispatch(stopGenerating()),
  }
}
