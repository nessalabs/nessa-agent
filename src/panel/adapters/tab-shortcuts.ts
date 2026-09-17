import type {
  ShortcutAction,
  ShortcutBinding,
  ShortcutsDocument,
  ShortcutSurface,
} from "@nessa/client"

import { chordMatches, parseAccelerator, type KeyChord } from "../../host/accelerator"
import { host } from "../../host"

export type FocusedPanelAction =
  | { action: "panel.newTab" }
  | { action: "panel.closeTab" }
  | { action: "panel.previousTab" | "panel.nextTab" }
  | { action: "panel.activateTab"; index?: number; conversationId?: string }

function surfaceMatches(
  bindingSurface: ShortcutSurface,
  surface: "desktop" | "browser",
): boolean {
  return bindingSurface === "*" || bindingSurface === surface
}

function toFocusedAction(binding: ShortcutBinding): FocusedPanelAction | null {
  const action = binding.action as ShortcutAction
  if (action === "panel.newTab") return { action: "panel.newTab" }
  if (action === "panel.closeTab") return { action: "panel.closeTab" }
  if (action === "panel.previousTab" || action === "panel.nextTab") return { action }
  if (action === "panel.activateTab") {
    const index =
      typeof binding.args?.index === "number" && Number.isInteger(binding.args.index)
        ? binding.args.index
        : undefined
    const conversationId =
      typeof binding.args?.conversationId === "string"
        ? binding.args.conversationId
        : undefined
    return { action: "panel.activateTab", index, conversationId }
  }
  return null
}

/**
 * Map a focused keydown to a panel tab action using the active shortcuts document.
 * Global summon is owned by the host; it is never matched here.
 */
export function matchFocusedShortcut(
  event: KeyChord,
  document: ShortcutsDocument,
  surface: "desktop" | "browser",
): FocusedPanelAction | null {
  if (event.repeat) return null

  for (const binding of document.bindings) {
    if (binding.scope !== "focused") continue
    if (!surfaceMatches(binding.surface, surface)) continue
    const parsed = parseAccelerator(binding.keys)
    if (!parsed || !chordMatches(event, parsed)) continue
    return toFocusedAction(binding)
  }
  return null
}

export function chordSurface(): "desktop" | "browser" {
  return host.kind === "browser" ? "browser" : "desktop"
}
