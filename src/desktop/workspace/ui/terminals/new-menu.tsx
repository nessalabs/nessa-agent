/**
 * A channel's +: a new session, and — where the window has terminals — a new
 * terminal in that channel, each with its key. Where there are no terminals
 * the + is the plain New Session button it always was.
 */
import { useState, type ComponentProps, type ReactNode } from "react"
import { newSession, newTerminal } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import { DesktopIcon } from "../../../ui/icons"
import { IconButton } from "../../../ui/icon-button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuItem,
  MenuShortcut,
} from "../../../ui/menu"
import { useWorkspaceFrame } from "../workspace-frame"
import { useTerminalSource } from "./terminal-source"

export function NewInChannelButton({
  channelId,
  label,
  icon = "add",
  ...button
}: {
  channelId?: string
  label: string
  icon?: ComponentProps<typeof IconButton>["icon"]
} & Omit<ComponentProps<typeof IconButton>, "icon" | "label" | "onClick">) {
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const terminals = useTerminalSource()
  const [open, setOpen] = useState(false)
  const startSession = () => dispatch(newSession(channelId ? { channelId } : {}))
  if (!terminals)
    return (
      <IconButton
        icon={icon}
        label={label}
        shortcut={frame.shortcut("newSession")}
        onClick={startSession}
        {...button}
      />
    )
  const shortcut = (label: string | undefined): ReactNode =>
    label ? <MenuShortcut>{label}</MenuShortcut> : null
  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <IconButton icon={icon} label={label} aria-expanded={open} {...button} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" sideOffset={6}>
        <MenuItem onSelect={startSession}>
          <DesktopIcon name="newSession" />
          New Session
          {shortcut(frame.shortcut("newSession"))}
        </MenuItem>
        <MenuItem
          data-new-terminal
          onSelect={() =>
            void dispatch(newTerminal(channelId ? { channelId } : {})).catch(
              (error: unknown) => console.warn("A terminal did not open", error),
            )
          }
        >
          <DesktopIcon name="terminal" />
          New Terminal
          {shortcut(frame.shortcut("newTerminal"))}
        </MenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
