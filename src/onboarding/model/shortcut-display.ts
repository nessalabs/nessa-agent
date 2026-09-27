import type { ShortcutsDocument } from "@nessa/client"

/** The keyboard conventions a shortcut is written in.
 *
 * `CmdOrCtrl` is one binding everywhere but three different things to read, so
 * the platform decides both the symbol and whether keys are joined or listed.
 */
export type ShortcutPlatform = "apple" | "windows" | "linux"

/** The accelerator the host actually registers for summoning the panel.
 *
 * This repeats the host's own rule — the first global `panel.summon` binding,
 * any surface (`shortcuts.rs::summon_accelerator`) — so what a person is told
 * is what is registered. A document with no global summon binding has no
 * accelerator to show, and the caller says so rather than inventing one.
 */
export function summonAccelerator(shortcuts: ShortcutsDocument): string | undefined {
  return shortcuts.bindings.find(
    (binding) => binding.action === "panel.summon" && binding.scope === "global",
  )?.keys
}

/** How each platform writes the modifiers an accelerator can name. */
const MODIFIERS: Readonly<Record<ShortcutPlatform, Readonly<Record<string, string>>>> =
  Object.freeze({
    apple: Object.freeze({
      cmdorctrl: "⌘",
      commandorcontrol: "⌘",
      cmd: "⌘",
      command: "⌘",
      super: "⌘",
      meta: "⌘",
      ctrl: "⌃",
      control: "⌃",
      alt: "⌥",
      option: "⌥",
      shift: "⇧",
    }),
    windows: Object.freeze({
      cmdorctrl: "Ctrl",
      commandorcontrol: "Ctrl",
      cmd: "Ctrl",
      command: "Ctrl",
      super: "Win",
      meta: "Win",
      ctrl: "Ctrl",
      control: "Ctrl",
      alt: "Alt",
      option: "Alt",
      shift: "Shift",
    }),
    linux: Object.freeze({
      cmdorctrl: "Ctrl",
      commandorcontrol: "Ctrl",
      cmd: "Ctrl",
      command: "Ctrl",
      super: "Super",
      meta: "Super",
      ctrl: "Ctrl",
      control: "Ctrl",
      alt: "Alt",
      option: "Alt",
      shift: "Shift",
    }),
  })

/** How each platform writes keys that have a name rather than a letter. */
const NAMED_KEYS: Readonly<Record<ShortcutPlatform, Readonly<Record<string, string>>>> =
  Object.freeze({
    apple: Object.freeze({
      enter: "↩",
      return: "↩",
      space: "Space",
      escape: "esc",
      esc: "esc",
      tab: "⇥",
      backspace: "⌫",
      delete: "⌦",
      up: "↑",
      down: "↓",
      left: "←",
      right: "→",
    }),
    windows: Object.freeze({ escape: "Esc", esc: "Esc" }),
    linux: Object.freeze({ escape: "Esc", esc: "Esc" }),
  })

/**
 * The spelling a table gives `token`, or nothing.
 *
 * `Object.hasOwn` rather than a plain read, because the token is whatever the
 * host's shortcut document wrote in an accelerator and a table is an object
 * like any other. `constructor` and `__proto__` survive `toLowerCase` intact
 * and every object answers to both, so a binding of `Cmd+constructor` read a
 * *function* out of this table and put it in a `string[]` — a keycap React
 * refuses to draw and an accessible name reading
 * `⌘function Object() { [native code] }`. Only a spelling the table owns is a
 * spelling.
 */
function spelling(
  table: Readonly<Record<string, string>>,
  token: string,
): string | undefined {
  return Object.hasOwn(table, token) ? table[token] : undefined
}

/**
 * Split an accelerator into the keys a person presses, written for `platform`.
 *
 * Each entry is one keycap. Tokens with no known spelling are passed through
 * with their own capitalisation rather than dropped, so an unrecognised
 * accelerator is still shown accurately instead of silently losing a modifier.
 */
export function acceleratorKeys(keys: string, platform: ShortcutPlatform): string[] {
  return keys
    .split("+")
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => {
      const token = part.toLowerCase()
      const written =
        spelling(MODIFIERS[platform], token) ?? spelling(NAMED_KEYS[platform], token)
      if (written) return written
      return part.length === 1 ? part.toUpperCase() : part
    })
}

/**
 * Write an accelerator as one string, the way its platform writes shortcuts:
 * `⌘⇧D` on Apple keyboards, `Ctrl+Shift+D` elsewhere. Used where a single label
 * is needed rather than keycaps — an accessible name, for instance.
 */
export function formatAccelerator(keys: string, platform: ShortcutPlatform): string {
  const written = acceleratorKeys(keys, platform)
  return platform === "apple" ? written.join("") : written.join("+")
}

/**
 * Which keys are down right now.
 *
 * Modifier state comes off any keyboard event, so this is knowable even for a
 * chord the window never receives whole — pressing Command alone is a key
 * event like any other, and only the completed accelerator is claimed by the
 * system.
 */
export interface HeldKeys {
  meta: boolean
  ctrl: boolean
  alt: boolean
  shift: boolean
  /** The non-modifier key held, lowercased, if there is one. */
  key?: string
}

/** Nothing held: the resting state, and what a surface starts from. */
export const NOTHING_HELD: HeldKeys = Object.freeze({
  meta: false,
  ctrl: false,
  alt: false,
  shift: false,
})

/** Whether a single accelerator token is currently down. Unknown tokens are
 * compared as plain keys, which is what they are. */
function tokenHeld(token: string, platform: ShortcutPlatform, held: HeldKeys): boolean {
  switch (token) {
    // The one binding that is two different keys depending on the keyboard.
    case "cmdorctrl":
    case "commandorcontrol":
    case "cmd":
    case "command":
      return platform === "apple" ? held.meta : held.ctrl
    case "super":
    case "meta":
      return held.meta
    case "ctrl":
    case "control":
      return held.ctrl
    case "alt":
    case "option":
      return held.alt
    case "shift":
      return held.shift
    default:
      return held.key === token
  }
}

/**
 * For each keycap of `keys`, whether that key is held down — in the same order
 * as [`acceleratorKeys`], so a caller can pair them off by index.
 *
 * This is what lets the step answer a person pressing the chord one key at a
 * time. Waiting for the whole accelerator tells someone who has Command down
 * and is hunting for Shift nothing at all, and the thing they most need to
 * know is that they have started correctly.
 */
export function heldAcceleratorKeys(
  keys: string,
  platform: ShortcutPlatform,
  held: HeldKeys,
): boolean[] {
  return keys
    .split("+")
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => tokenHeld(part.toLowerCase(), platform, held))
}
