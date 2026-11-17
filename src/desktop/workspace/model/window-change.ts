/**
 * A change one of the person's agents, or their own CLI, asks this window
 * to make (`workspace.command` on the gateway): what a click or a shortcut
 * here would do, carried out by the same rules
 * (`adapters/store/window-commands.ts`).
 */
import type { WidgetRef } from "../../widgets/model/widget-ref"

/** Where a conversation opens, relative to the focused pane. */
export type OpenPlace = "beside" | "right" | "below" | "here"

export type WindowChange =
  | { readonly kind: "sidebar" | "sessionList" | "panel"; readonly open: boolean }
  /** Brings this tab of the focused chat's panel to the front, opening the panel. */
  | { readonly kind: "panelTab"; readonly widget: WidgetRef }
  | { readonly kind: "show"; readonly view: "panes" | "agents" }
  | { readonly kind: "open"; readonly sessionId: string; readonly place: OpenPlace }
  | { readonly kind: "focus" | "close"; readonly sessionId: string }

/**
 * Why the window could not make a change: the conversation is not one it
 * lists; no pane shows it; the focused chat's panel has no such tab; this
 * arrangement has no session list; no chat is focused to have a panel; the
 * panes have no room on the side asked; something the person opened over it
 * keeps it from being drawn.
 */
export type WindowChangeRefusal =
  | "conversation_not_listed"
  | "not_on_screen"
  | "tab_not_found"
  | "no_session_list"
  | "no_chat_focused"
  | "no_room"
  | "covered"

export type WindowChangeOutcome =
  | { readonly kind: "applied" | "unchanged" }
  | { readonly kind: "refused"; readonly code: WindowChangeRefusal }
