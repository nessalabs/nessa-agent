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
