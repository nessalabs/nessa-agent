/**
 * App modes: Simple, the default, and Advanced, where the person picks the
 * features they want. A surface asks `useFeature(id)` and is told whether to
 * show it; it never asks which mode is in force.
 *
 * ```text
 *   Settings › Advanced › Mode ──┐
 *   (an agent, later, once the   ├──▶ changeFeatures ──▶ stored preference
 *    gateway's policy allows)  ──┘    (adapters/)        nessa.desktop.app-mode
 *                                                               │
 *   composer, …  ◀── useFeature(id) ◀── featureOn (model/) ◀────┘
 * ```
 *
 * `model/app-mode.ts` owns the feature table, what each mode shows, and the
 * one way the settings change; `adapters/app-mode-preference.ts` remembers
 * them and hands them to React. A kit component in nessa_ui never reads a
 * mode: the window passes it what to show.
 *
 * To add a feature, add a row to `appFeatures` and read it with
 * `useFeature` where it is drawn; Settings lists it on its own.
 */
export {
  appFeatures,
  featureOn,
  type AppFeature,
  type AppFeatureId,
  type AppMode,
  type FeatureChange,
  type FeatureSettings,
} from "./model/app-mode"
export {
  changeFeatures,
  useFeature,
  useFeatureSettings,
} from "./adapters/app-mode-preference"
