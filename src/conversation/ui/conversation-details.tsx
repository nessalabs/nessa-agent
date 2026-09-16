import { useState, type ReactElement } from "react"
import { Info, Pencil } from "lucide-react"
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
} from "@nessa-ui/react/context-menu"
import {
  AgentDetails,
  AgentDetailsSection,
  AgentDetailsField,
} from "@nessa-ui/react/agent-details"
import {
  Sheet,
  SheetHandle,
  SheetHeader,
  SheetExpand,
  SheetTitle,
  SheetAction,
  SheetBody,
} from "@nessa-ui/react/sheet"
import type { Conversation } from "../model"

export function ConversationTabMenu({
  children,
  onDetails,
  onRename,
}: {
  children: ReactElement
  onDetails: () => void
  onRename: () => void
}) {
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onSelect={onDetails}>
          <Info />
          View details
        </ContextMenuItem>
        <ContextMenuItem onSelect={onRename}>
          <Pencil />
          Rename
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}
export function ConversationDetails({
  conversation,
  rename,
  onClose,
  onRename,
}: {
  conversation: Conversation
  rename: boolean
  onClose: () => void
  onRename: (title: string) => void
}) {
  const [title, setTitle] = useState(conversation.title)
  const runtime = conversation.remote?.runtime
  return (
    <Sheet
      className="nessa-detail-sheet"
      label={rename ? "Rename conversation" : "Conversation details"}
      onClose={onClose}
    >
      <SheetHandle />
      <SheetHeader>
        <SheetExpand />
        <SheetTitle>{rename ? "Rename" : "Details"}</SheetTitle>
        <SheetAction>Done</SheetAction>
      </SheetHeader>
      <SheetBody>
        {rename ? (
          <form
            onSubmit={(event) => {
              event.preventDefault()
              if (title.trim()) {
                onRename(title.trim())
                onClose()
              }
            }}
            className="flex flex-col gap-4"
          >
            <label className="text-sm">
              Conversation name
              <input
                aria-label="Conversation name"
                value={title}
                maxLength={120}
                onChange={(event) => setTitle(event.target.value)}
                className="mt-2 w-full rounded-lg border p-3"
              />
            </label>
            <button
              type="submit"
              disabled={!title.trim()}
              className="rounded-full bg-primary px-4 py-2 text-primary-foreground disabled:opacity-50"
            >
              Save
            </button>
          </form>
        ) : (
          <AgentDetails title={conversation.title}>
            <AgentDetailsSection title="Info">
              {runtime ? (
                <>
                  <AgentDetailsField label="Runtime" value={runtime.provider} />
                  <AgentDetailsField label="Model" value={runtime.model} />
                  <AgentDetailsField
                    label="Working directory"
                    value={<span className="break-all">{runtime.workspace}</span>}
                  />
                </>
              ) : (
                <p className="text-sm text-muted-foreground">
                  Runtime details are not available yet.
                </p>
              )}
            </AgentDetailsSection>
          </AgentDetails>
        )}
      </SheetBody>
    </Sheet>
  )
}
