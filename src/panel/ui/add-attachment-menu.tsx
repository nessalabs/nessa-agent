import { useAttachmentMenu } from "./use-attachment-menu"
import { LogOut, Paperclip, Plus } from "lucide-react"
import { ChatComposerAction } from "@nessa-ui/react/chat-composer"
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuItem,
} from "@nessa-ui/react/dropdown-menu"

/** Keeps the Add menu above the complete composer, including attachment rows. */
export function AddAttachmentMenu({
  disabled,
  onChoose,
  onSignOut,
}: {
  disabled: boolean
  onChoose: () => void
  onSignOut?: () => void
}) {
  const { trigger, open, setOpen, offset } = useAttachmentMenu()
  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <ChatComposerAction
          className="nessa-composer-control"
          ref={trigger}
          aria-label={onSignOut ? "More options" : "Add attachment"}
          title={onSignOut ? "More options" : "Add attachment"}
          disabled={disabled && !onSignOut}
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
        <DropdownMenuItem
          disabled={disabled}
          className="focus-visible:outline-none"
          onSelect={onChoose}
        >
          <Paperclip aria-hidden="true" />
          Files
        </DropdownMenuItem>
        {onSignOut && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={onSignOut}>
              <LogOut aria-hidden="true" />
              Sign out
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
