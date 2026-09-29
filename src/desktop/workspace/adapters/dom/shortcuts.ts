/**
 * The workspace's keyboard: chords as data, bound to commands. A layout lists
 * the chords it binds to which commands (`ui/layouts/shortcuts.ts`); the
 * keyboard hook and every label read that one list, so a menu never names a
 * chord that does something else. How a chord is matched and written on this
 * platform is the window's (`src/desktop/model/keyboard.ts`), not this file's.
 */
import { useEffect, useRef } from "react"
import { isMac } from "../../../adapters/platform"
import { chordLabel, matchesChord, type Chord } from "../../../model/keyboard"

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
  return binding ? chordLabel(binding.chord, isMac) : undefined
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
      const binding = bindings.find((candidate) =>
        matchesChord(event, candidate.chord, isMac),
      )
      if (!binding) return
      // A handler returns false to leave the key to whatever else wants it.
      if (run(binding.command, event) === false) return
      event.preventDefault()
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [])
}
