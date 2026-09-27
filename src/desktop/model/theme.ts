/**
 * The window's light themes. A theme names only the colours of the ambient
 * light, the resize glow, and focus halos; each one's values live under its
 * `[data-desktop-theme]` block in `styles.css`.
 */
export const desktopThemes = [
  { id: "graphite", label: "Graphite" },
  { id: "ocean", label: "Ocean" },
  { id: "ember", label: "Ember" },
  { id: "dusk", label: "Dusk" },
] as const

export type DesktopThemeId = (typeof desktopThemes)[number]["id"]

export const defaultDesktopTheme: DesktopThemeId = "graphite"

/** Reads a stored or requested theme, falling back to the default for anything unknown. */
export function parseDesktopTheme(value: unknown): DesktopThemeId {
  return desktopThemes.find((theme) => theme.id === value)?.id ?? defaultDesktopTheme
}
