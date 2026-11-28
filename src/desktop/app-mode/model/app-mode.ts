/**
 * How much of the app a person sees. **Simple**, the default, shows a fixed,
 * calm set of features that nobody has to choose. **Advanced** lets the
 * person choose: each feature has a switch in Settings › Advanced › Mode.
 *
 * Every feature the mode can hide is one row of `appFeatures`: what Settings
 * calls it, whether Simple shows it, and whether Advanced shows it until the
 * person says otherwise. A surface asks only "is this feature on?"
 * (`featureOn`), never which mode is in force, so a feature can move between
 * modes, or a new mode can come, without touching what reads it.
 *
 * ```text
 *   FeatureChange ──▶ applyFeatureChange ──▶ FeatureSettings ──▶ featureOn(id)
 *   (Settings now,     the one way the           mode + the          what every
 *    an agent later)   settings change           person's picks      surface reads
 * ```
 *
 * The person's picks are kept while Simple is in force, so turning Advanced
 * back on brings them back; they apply only in Advanced.
 */
export const appModes = ["simple", "advanced"] as const

export type AppMode = (typeof appModes)[number]

interface AppFeatureShape {
  /** What surfaces ask for, and what the person's picks are stored under. */
  id: string
  /** Its name in Settings. */
  label: string
  /** The line under its name, where the name alone would leave someone unsure. */
  detail?: string
  /** Whether Simple shows it. Simple offers no choice. */
  simple: boolean
  /** Whether Advanced shows it until the person turns it the other way. */
  advanced: boolean
}

/** Every feature a mode can hide, in the order Settings lists them. */
export const appFeatures = [
  {
    id: "composer-model-picker",
    label: "Model in the composer",
    detail: "The model and Fast in the composer’s row. Off, they are under +.",
    simple: false,
    advanced: true,
  },
] as const satisfies readonly AppFeatureShape[]

export type AppFeature = (typeof appFeatures)[number]
export type AppFeatureId = AppFeature["id"]

/** The mode in force, and the features the person turned the other way from Advanced's default. */
export interface FeatureSettings {
  readonly mode: AppMode
  /** Only what differs from Advanced's default: a pick that matches it is not kept. */
  readonly picks: Readonly<Partial<Record<AppFeatureId, boolean>>>
}

export const defaultFeatureSettings: FeatureSettings = { mode: "simple", picks: {} }

/**
 * Every way the settings change, from Settings today and from an agent
 * later; there is no other. An agent's change will reach the window only
 * after the gateway's Cedar policy allows it (Gate 18), and is applied here
 * like a person's.
 */
export type FeatureChange =
  | { readonly kind: "mode"; readonly mode: AppMode }
  | { readonly kind: "feature"; readonly id: AppFeatureId; readonly on: boolean }

function feature(id: AppFeatureId): AppFeature {
  const found = appFeatures.find((each) => each.id === id)
  // Ids are typed from the table itself, so a miss is a table that contradicts its own types.
  if (!found) throw new Error(`no app feature ${id}`)
  return found
}

/** Whether `id` names a feature in this build. */
export function isAppFeature(value: unknown): value is AppFeatureId {
  return typeof value === "string" && appFeatures.some((each) => each.id === value)
}

/** Whether the feature shows: Simple's fixed answer, or the person's pick in Advanced. */
export function featureOn(settings: FeatureSettings, id: AppFeatureId): boolean {
  const { simple, advanced } = feature(id)
  if (settings.mode === "simple") return simple
  return Object.hasOwn(settings.picks, id) ? settings.picks[id] === true : advanced
}

/**
 * The settings after `change`, as a new value. A feature turned to match
 * Advanced's default drops its pick, so a default changed in a later build
 * reaches everyone who never chose otherwise.
 */
export function applyFeatureChange(
  settings: FeatureSettings,
  change: FeatureChange,
): FeatureSettings {
  switch (change.kind) {
    case "mode":
      return { ...settings, mode: change.mode }
    case "feature": {
      const { [change.id]: _previous, ...rest } = settings.picks
      return {
        ...settings,
        picks:
          change.on === feature(change.id).advanced
            ? rest
            : { ...rest, [change.id]: change.on },
      }
    }
  }
}

/**
 * Reads stored settings from whatever storage held. Anything this build does
 * not know — another mode, a feature since removed, a pick that is not true
 * or false — is dropped on its own, so one bad entry never costs the rest,
 * and nothing stored reads as the default.
 */
export function parseFeatureSettings(stored: unknown): FeatureSettings {
  if (typeof stored !== "string") return defaultFeatureSettings
  let value: unknown
  try {
    value = JSON.parse(stored)
  } catch {
    return defaultFeatureSettings
  }
  if (typeof value !== "object" || value === null) return defaultFeatureSettings
  const record = value as Record<string, unknown>
  const mode =
    appModes.find((each) => each === record.mode) ?? defaultFeatureSettings.mode
  const picks: Partial<Record<AppFeatureId, boolean>> = {}
  const storedPicks = record.picks
  if (typeof storedPicks === "object" && storedPicks !== null)
    for (const { id, advanced } of appFeatures) {
      if (!Object.hasOwn(storedPicks, id)) continue
      const on = (storedPicks as Record<string, unknown>)[id]
      if (typeof on === "boolean" && on !== advanced) picks[id] = on
    }
  return { mode, picks }
}

export function serializeFeatureSettings(settings: FeatureSettings): string {
  return JSON.stringify({ mode: settings.mode, picks: settings.picks })
}
