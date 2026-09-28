import { useLayoutEffect } from "react"
import { parseDesktopTheme } from "../model/theme"
import { storedPreference } from "./stored-preference"

/**
 * The chosen theme, remembered in this webview's storage and shared by every
 * surface that reads it, so Settings and the window change together.
 */
const themePreference = storedPreference({
  key: "nessa.desktop.theme",
  event: "nessa:desktop-theme",
  parse: parseDesktopTheme,
})

export const useThemePreference = themePreference.usePreference

/**
 * Carries the chosen theme on the page's root as `data-desktop-theme`, before
 * the first paint and whenever it changes. Each surface wears the theme
 * itself; this is for what portals to the body, outside every surface —
 * menus and pickers — so their highlight takes the theme's light too. A
 * picture's tint (`ui/header-art.tsx`) stays on the surface it tints.
 * Mounted once, by the window.
 */
export function useThemeOnDocument(): void {
  const [theme] = useThemePreference()
  useLayoutEffect(() => {
    document.documentElement.dataset.desktopTheme = theme
  }, [theme])
}
