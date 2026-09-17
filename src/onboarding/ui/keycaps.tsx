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
}: {
  keys: string
  platform: ShortcutPlatform
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
          className="nessa-keycap flex min-w-11 items-center justify-center rounded-xl px-3 py-2.5 font-sans nessa-text-5 font-semibold text-neutral-900"
        >
          {cap}
        </kbd>
      ))}
    </span>
  )
}
