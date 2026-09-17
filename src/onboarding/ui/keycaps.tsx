import type { ShortcutPlatform } from "../model/shortcut-display"
import { acceleratorKeys, formatAccelerator } from "../model/shortcut-display"

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
  pressed = false,
}: {
  keys: string
  platform: ShortcutPlatform
  /** Whether the shortcut has been pressed. The caps light and stay lit, so
   * the press is visibly what opened the way on. */
  pressed?: boolean
}) {
  const caps = acceleratorKeys(keys, platform)
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
          data-pressed={pressed || undefined}
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
