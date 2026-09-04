import type {
  ShortcutAction,
  ShortcutBinding,
  ShortcutsDocument,
  ShortcutSurface,
} from "@nessa/client"

import { host } from "../../host"

export type FocusedPanelAction =
  | { action: "panel.newTab" }
  | { action: "panel.closeTab" }
  | { action: "panel.activateTab"; index?: number; conversationId?: string }

type KeyChord = {
  key: string
  code: string
  metaKey: boolean
  ctrlKey: boolean
  altKey: boolean
  shiftKey: boolean
  repeat: boolean
}

type ParsedKeys = {
  cmdOrCtrl: boolean
  meta: boolean
  ctrl: boolean
  alt: boolean
  shift: boolean
  key: string
}

/**
 * Parse a Tauri-style accelerator (`CmdOrCtrl+Shift+T`) into modifiers + key.
 * Returns null when the string is empty or has no key token.
 */
export function parseAccelerator(keys: string): ParsedKeys | null {
  const parts = keys
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean)
  if (parts.length === 0) return null

  const keyToken = parts[parts.length - 1]!
  let cmdOrCtrl = false
  let meta = false
  let ctrl = false
  let alt = false
  let shift = false

  for (const part of parts.slice(0, -1)) {
    const token = part.toLowerCase()
    if (token === "cmdorctrl" || token === "commandorcontrol") cmdOrCtrl = true
    else if (token === "cmd" || token === "command" || token === "super" || token === "meta")
      meta = true
    else if (token === "ctrl" || token === "control") ctrl = true
    else if (token === "alt" || token === "option") alt = true
    else if (token === "shift") shift = true
    else return null
  }

  return {
    cmdOrCtrl,
    meta,
    ctrl,
    alt,
    shift,
    key: keyToken.length === 1 ? keyToken.toLowerCase() : keyToken.toLowerCase(),
  }
}

function chordMatches(event: KeyChord, parsed: ParsedKeys): boolean {
  if (event.altKey !== parsed.alt) return false
  if (event.shiftKey !== parsed.shift) return false

  const wantsMeta = parsed.cmdOrCtrl || parsed.meta
  const wantsCtrl = parsed.cmdOrCtrl || parsed.ctrl
  if (parsed.cmdOrCtrl) {
    if (!(event.metaKey || event.ctrlKey)) return false
  } else {
    if (event.metaKey !== wantsMeta) return false
    if (event.ctrlKey !== wantsCtrl) return false
  }

  const key = event.key.length === 1 ? event.key.toLowerCase() : event.key.toLowerCase()
  if (key === parsed.key) return true

  // Digits: prefer `code` so Shift+1 still matches binding key "1" on some layouts.
  if (/^\d$/.test(parsed.key)) {
    return event.code === `Digit${parsed.key}` || event.code === `Numpad${parsed.key}`
  }
  return false
}

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
