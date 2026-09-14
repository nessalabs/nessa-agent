import { useAttachmentMenu } from "./use-attachment-menu"
import { Paperclip, Plus } from "lucide-react"
import { ChatComposerAction } from "@nessa-ui/react/chat-composer"
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuItem,
} from "@nessa-ui/react/dropdown-menu"

/** Keeps the Add menu above the complete composer, including attachment rows. */
export function AddAttachmentMenu({
  disabled,
  onChoose,
}: {
  disabled: boolean
  onChoose: () => void
}) {
  const { trigger, open, setOpen, offset } = useAttachmentMenu()
  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <ChatComposerAction
          className="nessa-composer-control"
          ref={trigger}
          aria-label="Add attachment"
          title="Add attachment"
          disabled={disabled}
        >
          <Plus aria-hidden="true" />
        </ChatComposerAction>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        side="top"
        align="start"
        sideOffset={offset}
        className="min-w-56"
      >
        <DropdownMenuLabel>Add</DropdownMenuLabel>
        <DropdownMenuItem className="focus-visible:outline-none" onSelect={onChoose}>
          <Paperclip aria-hidden="true" />
          Files
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
