import type { ShortcutsDocument } from "@nessa/client"
import type { HostKind } from "../../host/features"

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

/** Modifier glyphs Apple platforms are written with. */
const APPLE_GLYPHS: Readonly<Record<string, string>> = Object.freeze({
  cmdorctrl: "⌘",
  cmd: "⌘",
  command: "⌘",
  super: "⌘",
  ctrl: "⌃",
  control: "⌃",
  alt: "⌥",
  option: "⌥",
  shift: "⇧",
})

/** Modifier words every other platform is written with. */
const NAMES: Readonly<Record<string, string>> = Object.freeze({
  cmdorctrl: "Ctrl",
  cmd: "Ctrl",
  command: "Ctrl",
  super: "Super",
  ctrl: "Ctrl",
  control: "Ctrl",
  alt: "Alt",
  option: "Alt",
  shift: "Shift",
})

/**
 * Write a Tauri-style accelerator the way the host platform writes shortcuts:
 * `⌘⇧D` on macOS, `Ctrl+Shift+D` elsewhere.
 *
 * `CmdOrCtrl` is the same binding on every platform but is not written the same
 * way, which is the whole reason this exists. Tokens with no known spelling are
 * passed through unchanged rather than dropped, so an unrecognised accelerator
 * is still shown accurately instead of silently losing a modifier.
 */
export function formatAccelerator(keys: string, host: HostKind): string {
  const apple = host === "macos"
  const parts = keys
    .split("+")
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
  if (parts.length === 0) return ""
  const written = parts.map((part) => {
    const lookup = (apple ? APPLE_GLYPHS : NAMES)[part.toLowerCase()]
    return lookup ?? (part.length === 1 ? part.toUpperCase() : part)
  })
  return apple ? written.join("") : written.join("+")
}
