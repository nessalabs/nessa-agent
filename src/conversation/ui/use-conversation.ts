import { pollConversation } from "../adapters/gateway/polling"
import { useEffect, useEffectEvent, useState } from "react"
import { type FileAttachment, type MessageContent } from "../model"
import { activeConversation } from "../application/queries/active-conversation"
import {
  renameConversation,
  attachFiles,
  removeFile,
  closeConversation,
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
    conversations,
    active,
    gatewayAvailable,
    moveActive: (direction: -1 | 1) => dispatch(moveActive(direction)),
    setActive: (id: string) => dispatch(setActive(id)),
    submit: (content: MessageContent) => {
      if (!gatewayAvailable) return
      void dispatch(
        sendDraft({
          content,
          id: active.id,
          steering: deliveryMode === "steer" && active.phase !== "idle",
        }),
      )
    },
    openConversation: () => {
      dispatch(openConversation())
    },
    closeConversation: (id: string) => dispatch(closeConversation(id)),
    setDraft: (draft: MessageContent) => dispatch(setDraft({ draft })),
    stopGenerating: () => dispatch(stopGenerating()),
  }
}
