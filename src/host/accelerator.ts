/**
 * The host's accelerator grammar, shared by everything that has to understand
 * it: the panel matches focused chords against the shortcut document, and setup
 * teaches the summon binding and waits for the person to press it.
 *
 * One grammar parsed in two places would drift, so it is parsed here once.
 */
export type KeyChord = {
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

  const keyToken = parts[parts.length - 1]
  if (keyToken === undefined) return null
  let cmdOrCtrl = false
  let meta = false
  let ctrl = false
  let alt = false
  let shift = false

  for (const part of parts.slice(0, -1)) {
    const token = part.toLowerCase()
    if (token === "cmdorctrl" || token === "commandorcontrol") cmdOrCtrl = true
    else if (
      token === "cmd" ||
      token === "command" ||
      token === "super" ||
      token === "meta"
    )
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

export function chordMatches(event: KeyChord, parsed: ParsedKeys): boolean {
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

/** Whether a keyboard event is the chord this accelerator describes. */
export function matchesAccelerator(event: KeyChord, keys: string | undefined): boolean {
  if (!keys) return false
  const parsed = parseAccelerator(keys)
  return parsed ? chordMatches(event, parsed) : false
}
