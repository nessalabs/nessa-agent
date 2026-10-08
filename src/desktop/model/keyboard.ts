/**
 * The window's keyboard, as a platform writes and presses it: the one owner
 * of how a chord is matched against a key event and written out for a menu
 * or tooltip, on a Mac and elsewhere. Every surface of the window — the
 * workspace (`workspace/adapters/dom/shortcuts.ts`), the classic shell
 * (`ui/desktop-app.tsx`) and Settings — states its chords as data and asks
 * here, so no label can name a key the binding does not take.
 *
 * Pure: which platform it is, is the caller's to say. The window reads it
 * once (`adapters/platform.ts`).
 */

/** A key and its modifiers. `command` is ⌘ on a Mac and Control elsewhere; `control` is always Control. */
export interface Chord {
  /** A `KeyboardEvent.code`, so the chord holds whatever the keyboard layout. */
  readonly code: string
  readonly command?: boolean
  readonly shift?: boolean
  readonly alt?: boolean
  readonly control?: boolean
}

/** What of a key event a chord is matched against. */
export type ChordEvent = Pick<
  KeyboardEvent,
  "code" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey"
>

/** Whether a browser's user agent is a Mac's, whose keyboard has ⌘ and ⌥. */
export function macUserAgent(userAgent: string): boolean {
  return /Mac/.test(userAgent)
}

/** Whether the platform's command key is down: ⌘ on a Mac, Control elsewhere. */
export function commandKey(
  event: { metaKey: boolean; ctrlKey: boolean },
  mac: boolean,
): boolean {
  return mac ? event.metaKey : event.ctrlKey
}

export function matchesChord(event: ChordEvent, chord: Chord, mac: boolean): boolean {
  if (event.code !== chord.code) return false
  if (event.shiftKey !== Boolean(chord.shift) || event.altKey !== Boolean(chord.alt))
    return false
  // On a Mac, Control is a modifier of its own; elsewhere it is the command key too.
  if (mac)
    return (
      event.metaKey === Boolean(chord.command) && event.ctrlKey === Boolean(chord.control)
    )
  return !event.metaKey && event.ctrlKey === Boolean(chord.command || chord.control)
}

/** The event a chord asks for on a platform: which physical modifiers are down. */
export function chordEvent(chord: Chord, mac: boolean): ChordEvent {
  return {
    code: chord.code,
    metaKey: mac && Boolean(chord.command),
    ctrlKey: mac ? Boolean(chord.control) : Boolean(chord.command || chord.control),
    shiftKey: Boolean(chord.shift),
    altKey: Boolean(chord.alt),
  }
}

const keyNames: Record<string, string> = {
  Backslash: "\\",
  BracketLeft: "[",
  BracketRight: "]",
  Comma: ",",
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  ArrowDown: "↓",
  Enter: "↩",
  Backspace: "⌫",
}

function keyName(code: string): string {
  if (Object.hasOwn(keyNames, code)) return keyNames[code]
  if (code.startsWith("Key")) return code.slice(3)
  if (code.startsWith("Digit")) return code.slice(5)
  return code
}

/**
 * A chord as the platform writes it: ⌃⌥⇧⌘ on a Mac, Ctrl+Alt+Shift elsewhere
 * — Option is ⌥ on a Mac and Alt everywhere else (`keyboard.test.ts`).
 */
export function chordLabel(chord: Chord, mac: boolean): string {
  const key = keyName(chord.code)
  if (mac)
    return `${chord.control ? "⌃" : ""}${chord.alt ? "⌥" : ""}${chord.shift ? "⇧" : ""}${chord.command ? "⌘" : ""}${key}`
  const parts = [
    chord.command || chord.control ? "Ctrl" : "",
    chord.alt ? "Alt" : "",
    chord.shift ? "Shift" : "",
    key,
  ]
  return parts.filter(Boolean).join("+")
}

/**
 * A chord as `aria-keyshortcuts` names it — modifiers, then the key's
 * `KeyboardEvent.key` value, joined by "+": "Meta+Enter" on a Mac,
 * "Control+Enter" elsewhere (`keyboard.test.ts`).
 */
export function chordShortcut(chord: Chord, mac: boolean): string {
  const key = chord.code.startsWith("Key")
    ? chord.code.slice(3)
    : chord.code.startsWith("Digit")
      ? chord.code.slice(5)
      : chord.code === "NumpadEnter"
        ? "Enter"
        : chord.code
  return [
    chord.control || (chord.command && !mac) ? "Control" : "",
    chord.alt ? "Alt" : "",
    chord.shift ? "Shift" : "",
    chord.command && mac ? "Meta" : "",
    key,
  ]
    .filter(Boolean)
    .join("+")
}

/** The command key alone, as the platform writes it before a click: "⌘Click", "Ctrl+Click". */
export function commandLabel(mac: boolean): string {
  return mac ? "⌘" : "Ctrl+"
}
