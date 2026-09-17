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
      const written = MODIFIERS[platform][token] ?? NAMED_KEYS[platform][token]
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
