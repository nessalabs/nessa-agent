/**
 * The workspace's keyboard: chords as data, matched against key events and
 * written out for menus and tooltips. A layout lists the chords it binds to
 * which commands (`ui/layouts/shortcuts.ts`); the keyboard hook and every
 * label read that one list, so a menu never names a chord that does
 * something else.
 */
import { useEffect, useRef } from "react"

export const isMac = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent)

/** The platform's command key: ⌘ on a Mac, Control elsewhere. */
export const commandKey = (event: { metaKey: boolean; ctrlKey: boolean }): boolean =>
  isMac ? event.metaKey : event.ctrlKey

/** A key and its modifiers. `command` is ⌘ on a Mac and Control elsewhere; `control` is always Control. */
export interface Chord {
  /** A `KeyboardEvent.code`, so the chord holds whatever the keyboard layout. */
  readonly code: string
  readonly command?: boolean
  readonly shift?: boolean
  readonly alt?: boolean
  readonly control?: boolean
}

export function matchesChord(
  event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">,
  chord: Chord,
  mac = isMac,
): boolean {
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
export function chordEvent(
  chord: Chord,
  mac = isMac,
): Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey"> {
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
}

function keyName(code: string): string {
  if (Object.hasOwn(keyNames, code)) return keyNames[code]
  if (code.startsWith("Key")) return code.slice(3)
  if (code.startsWith("Digit")) return code.slice(5)
  return code
}

/** A chord as the platform writes it: ⌃⌥⇧⌘ on a Mac, Ctrl+Alt+Shift elsewhere. */
export function chordLabel(chord: Chord, mac = isMac): string {
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

/** The command key alone, as the platform writes it, for "⌘Click". */
export const commandLabel = isMac ? "⌘" : "Ctrl+"

export interface Binding<Command extends string> {
  readonly chord: Chord
  readonly command: Command
}

/** The label of the first chord bound to `command`, for a menu or tooltip. */
export function labelOf<Command extends string>(
  bindings: readonly Binding<Command>[],
  command: Command,
): string | undefined {
  const binding = bindings.find((candidate) => candidate.command === command)
  return binding ? chordLabel(binding.chord) : undefined
}

/**
 * Runs the command a window-wide key event is bound to. The listener is bound
 * once and reads the latest `run` through a ref, so a render never rebinds it.
 */
export function useKeyBindings<Command extends string>(
  bindings: readonly Binding<Command>[],
  run: (command: Command, event: KeyboardEvent) => boolean | void,
): void {
  const latest = useRef({ bindings, run })
  latest.current = { bindings, run }
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const { bindings, run } = latest.current
      const binding = bindings.find((candidate) => matchesChord(event, candidate.chord))
      if (!binding) return
      // A handler returns false to leave the key to whatever else wants it.
      if (run(binding.command, event) === false) return
      event.preventDefault()
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [])
}
