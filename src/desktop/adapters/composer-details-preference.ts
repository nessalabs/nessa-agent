import { parseComposerDetails } from "../model/composer-details"
import { storedPreference } from "./stored-preference"

/**
 * Whether the line under a composer shows the project and the computer
 * (`model/composer-details.ts`), remembered like the window's other
 * preferences and shared by every composer, so hiding it under one hides it
 * under all.
 */
const composerDetailsPreference = storedPreference({
  key: "nessa.desktop.composer-details",
  event: "nessa:desktop-composer-details",
  parse: parseComposerDetails,
})

export const useComposerDetailsPreference = composerDetailsPreference.usePreference
