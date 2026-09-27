import type { ShortcutPlatform } from "../model/shortcut-display"
import {
  acceleratorKeys,
  formatAccelerator,
  heldAcceleratorKeys,
  NOTHING_HELD,
  type HeldKeys,
} from "../model/shortcut-display"

/**
 * A shortcut drawn as the keys a person presses.
 *
 * Each key is its own cap, so the shortcut reads as something to do rather than
 * a string to decode. The caps are decorative in the accessibility tree: the
 * group carries the shortcut as one label, because a screen reader announcing
 * six separate glyphs is worse than one spoken shortcut.
 */
export function Keycaps({
  keys,
  platform,
  pressed,
  held = NOTHING_HELD,
}: {
  keys: string
  platform: ShortcutPlatform
  /** Which keys are down right now. Each cap lights as its own key goes down,
   * so pressing the chord one key at a time is answered at every step rather
   * than only when the whole thing lands. */
  held?: HeldKeys
  /** What the shortcut last did, if it has been pressed. The caps light on
   * each press, so every press is visibly received. */
  pressed?: "shown" | "hidden"
}) {
  const caps = acceleratorKeys(keys, platform)
  const down = heldAcceleratorKeys(keys, platform, held)
  if (caps.length === 0) return null
  return (
    <span
      role="img"
      aria-label={formatAccelerator(keys, platform)}
      className="nessa-setup-arrive flex items-center gap-2"
    >
      {caps.map((cap, index) => (
        <kbd
          // Caps repeat within a shortcut, so position is the only identity.
          key={`${cap}-${index}`}
          aria-hidden="true"
          data-pressed={pressed}
          data-held={down[index] || undefined}
          // Square at a single glyph and wider at a word, the way a keyboard
          // sizes its own caps.
          className="nessa-keycap flex h-14 min-w-14 items-center justify-center px-4 font-sans nessa-text-5 font-semibold"
        >
          {cap}
        </kbd>
      ))}
    </span>
  )
}
