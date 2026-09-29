import { parseIconFamily } from "../model/icon-family"
import { storedPreference } from "./stored-preference"

/**
 * The chosen icon family, remembered in this webview's storage and shared by
 * every surface that reads it, so Settings and the window's chrome change
 * together.
 */
const iconFamilyPreference = storedPreference({
  key: "nessa.desktop.icon-family",
  event: "nessa:desktop-icon-family",
  parse: parseIconFamily,
})

export const useIconFamilyPreference = iconFamilyPreference.usePreference
