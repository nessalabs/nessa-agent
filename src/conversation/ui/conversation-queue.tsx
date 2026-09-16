import { useState } from "react"
import {
  Sheet,
  SheetHandle,
  SheetHeader,
  SheetTitle,
  SheetExpand,
  SheetAction,
  SheetBody,
} from "@nessa-ui/react/sheet"
import {
  ComposerQueueBadge,
  ComposerQueue,
  ComposerQueueItem,
} from "@nessa-ui/react/composer-queue"
import { type Conversation } from "../model"
import { useConversationDispatch } from "../adapters/store/hooks"
import { controlConversation } from "../adapters/store/slice"

/** Waiting messages belong beside the composer until the SDK dispatches them. */
export function ConversationQueue({
  conversation,
  gatewayAvailable,
}: {
  conversation: Conversation
  gatewayAvailable: boolean
}) {
  const [open, setOpen] = useState(false)
  const dispatch = useConversationDispatch()
  const waiting = conversation.remote?.pending ?? []
  if (!waiting.length) return null
  return (
    <>
      <ComposerQueueBadge
        count={waiting.length}
        className="mb-2"
        aria-expanded={open}
        onClick={() => setOpen(true)}
      />
      {open && (
        <Sheet
          className="nessa-detail-sheet"
          label="Queued messages"
          onClose={() => setOpen(false)}
        >
          <SheetHandle />
          <SheetHeader>
            <SheetExpand />
            <SheetTitle>Queued</SheetTitle>
            <SheetAction>Done</SheetAction>
          </SheetHeader>
          <SheetBody>
            <ComposerQueue
              appearance="plain"
              itemIds={waiting.map((turn) => turn.executionId)}
              onReorder={(executionIds) => {
                if (
                  !gatewayAvailable ||
                  conversation.controlPending ||
                  !conversation.remote?.queueComplete
                )
                  return
                void dispatch(
                  controlConversation({
                    id: conversation.id,
                    control: { kind: "reorder", executionIds },
                  }),
                )
              }}
            >
              {waiting.map((turn) => (
                <ComposerQueueItem
                  key={turn.executionId}
                  id={turn.executionId}
                  itemLabel={turn.text}
                  showHandle={
                    gatewayAvailable &&
                    !conversation.controlPending &&
                    !!conversation.remote?.queueComplete
                  }
                  onPromote={
                    !gatewayAvailable ||
                    conversation.controlPending ||
                    !conversation.remote?.queueComplete
                      ? undefined
                      : () => {
                          const peers = waiting.filter(
                            (item) => item.executionId !== turn.executionId,
                          )
                          const ordered =
                            turn.mode === "steering"
                              ? [turn, ...peers]
                              : [
                                  ...peers.filter((item) => item.mode === "steering"),
                                  turn,
                                  ...peers.filter((item) => item.mode !== "steering"),
                                ]
                          void dispatch(
                            controlConversation({
                              id: conversation.id,
                              control: {
                                kind: "reorder",
                                executionIds: ordered.map((item) => item.executionId),
                              },
                            }),
                          )
                        }
                  }
                  onRemove={
                    !gatewayAvailable || conversation.controlPending
                      ? undefined
                      : () => {
                          void dispatch(
                            controlConversation({
                              id: conversation.id,
                              control: { kind: "remove", executionId: turn.executionId },
                            }),
                          )
                        }
                  }
                >
                  <span>
                    {turn.text}
                    {turn.mode === "steering" && (
                      <span className="block text-xs text-muted-foreground">
                        Steering
                      </span>
                    )}
                  </span>
                </ComposerQueueItem>
              ))}
            </ComposerQueue>
          </SheetBody>
        </Sheet>
      )}
    </>
  )
}
