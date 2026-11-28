import {
  applyFeatureChange,
  featureOn,
  parseFeatureSettings,
  serializeFeatureSettings,
  type AppFeatureId,
  type FeatureChange,
  type FeatureSettings,
} from "../model/app-mode"
import { storedPreference } from "../../adapters/stored-preference"

/**
 * The mode and the person's picks, remembered like the window's other
 * preferences (`stored-preference.ts`): kept across restarts, and a change
 * in one window reaches every reader in it and in the windows beside it.
 */
const appModePreference = storedPreference<FeatureSettings>({
  key: "nessa.desktop.app-mode",
  event: "nessa:desktop-app-mode",
  parse: parseFeatureSettings,
  serialize: serializeFeatureSettings,
})

/**
 * Changes the mode or a feature: the one entry point for every change,
 * worked out from the settings in force now (`applyFeatureChange`).
 */
export function changeFeatures(change: FeatureChange): void {
  appModePreference.choose(applyFeatureChange(appModePreference.current(), change))
}

/** The mode and the person's picks, for Settings; a surface asks `useFeature`. */
export function useFeatureSettings(): FeatureSettings {
  return appModePreference.usePreference()[0]
}

/** Whether `id` shows, in whichever mode is in force; follows every change. */
export function useFeature(id: AppFeatureId): boolean {
  return featureOn(useFeatureSettings(), id)
}
