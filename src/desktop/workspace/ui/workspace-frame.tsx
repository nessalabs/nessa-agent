/**
 * What a layout lends the components it arranges: the chords it binds (for
 * menu labels), how to measure a pane's room before a split, its quick
 * switcher if it has one, and which channel its session list shows, so a
 * conversation's heading need not repeat it. The frame's functions keep
 * their identity for the layout's life; the listed channel is its own
 * context, so a change of view renders only the headings.
 */
import { createContext, useContext } from "react"
import type { PaneKey } from "../model/pane-layout"
import type { PaneRoom } from "../model/pane-sizing"

/** Everything a key or a menu can ask of a layout. */
export type ShortcutCommand =
  | "toggleSidebar"
  | "toggleSessionList"
  | "newSession"
  | "newSessionBeside"
  | "splitRight"
  | "splitDown"
  | "closePane"
  | "search"
  | "switcher"
  | "openBeside"
  | "focusPane1"
  | "focusPane2"
  | "focusPane3"
  | "focusPane4"
  | "focusPrevious"
  | "focusNext"
  | "moveLeft"
  | "moveRight"
  | "moveUp"
  | "moveDown"

export interface WorkspaceFrame {
  /** The chord bound to a command in this layout, written out; none when it has none. */
  shortcut(command: ShortcutCommand): string | undefined
  /** A pane's room for a split, the sidebar's included; nothing when it is not on screen. */
  roomOf(pane: PaneKey): PaneRoom | undefined
  /** Opens this layout's quick switcher; absent in a layout without one. */
  openSwitcher?: (mode: "open" | "split") => void
  /** Brings a session's row into view and focus, once a reveal has opened its channel. */
  showRow(sessionId: string): void
}

const FrameContext = createContext<WorkspaceFrame>({
  shortcut: () => undefined,
  roomOf: () => undefined,
  showRow: () => undefined,
})

export const WorkspaceFrameProvider = FrameContext.Provider
export const useWorkspaceFrame = () => useContext(FrameContext)

/** The channel the session list shows beside the panes, if it shows one. */
const ListedChannelContext = createContext<string | null>(null)
export const ListedChannelProvider = ListedChannelContext.Provider
export const useListedChannel = () => useContext(ListedChannelContext)
