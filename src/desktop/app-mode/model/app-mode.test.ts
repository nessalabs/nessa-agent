import { describe, expect, it } from "vitest"
import {
  appFeatures,
  applyFeatureChange,
  defaultFeatureSettings,
  featureOn,
  isAppFeature,
  parseFeatureSettings,
  serializeFeatureSettings,
  type FeatureSettings,
} from "./app-mode"

const advanced: FeatureSettings = { mode: "advanced", picks: {} }

describe("app modes", () => {
  it("starts in Simple, which shows each feature as the table says", () => {
    expect(defaultFeatureSettings.mode).toBe("simple")
    for (const each of appFeatures)
      expect(featureOn(defaultFeatureSettings, each.id)).toBe(each.simple)
  })

  it("shows Advanced's default until the person picks otherwise", () => {
    expect(featureOn(advanced, "composer-model-picker")).toBe(true)
    const off = applyFeatureChange(advanced, {
      kind: "feature",
      id: "composer-model-picker",
      on: false,
    })
    expect(featureOn(off, "composer-model-picker")).toBe(false)
    expect(advanced.picks).toEqual({})
  })

  it("keeps the picks while Simple is in force and brings them back with Advanced", () => {
    const picked = applyFeatureChange(advanced, {
      kind: "feature",
      id: "composer-model-picker",
      on: false,
    })
    const simple = applyFeatureChange(picked, { kind: "mode", mode: "simple" })
    expect(featureOn(simple, "composer-model-picker")).toBe(false)
    expect(simple.picks).toEqual({ "composer-model-picker": false })
    const back = applyFeatureChange(simple, { kind: "mode", mode: "advanced" })
    expect(featureOn(back, "composer-model-picker")).toBe(false)
  })

  it("drops a pick that matches Advanced's default", () => {
    const off = applyFeatureChange(advanced, {
      kind: "feature",
      id: "composer-model-picker",
      on: false,
    })
    const on = applyFeatureChange(off, {
      kind: "feature",
      id: "composer-model-picker",
      on: true,
    })
    expect(on.picks).toEqual({})
  })

  it("reads back what it stored", () => {
    const settings: FeatureSettings = {
      mode: "advanced",
      picks: { "composer-model-picker": false },
    }
    expect(parseFeatureSettings(serializeFeatureSettings(settings))).toEqual(settings)
  })

  it("falls back entry by entry on what this build does not know", () => {
    expect(parseFeatureSettings(null)).toEqual(defaultFeatureSettings)
    expect(parseFeatureSettings("not json")).toEqual(defaultFeatureSettings)
    expect(parseFeatureSettings('"advanced"')).toEqual(defaultFeatureSettings)
    expect(
      parseFeatureSettings(
        JSON.stringify({
          mode: "expert",
          picks: { gone: true, "composer-model-picker": "no", toString: false },
        }),
      ),
    ).toEqual(defaultFeatureSettings)
    expect(
      parseFeatureSettings(
        JSON.stringify({ mode: "advanced", picks: { "composer-model-picker": false } }),
      ),
    ).toEqual({ mode: "advanced", picks: { "composer-model-picker": false } })
  })

  it("names only the features in its table", () => {
    expect(isAppFeature("composer-model-picker")).toBe(true)
    expect(isAppFeature("toString")).toBe(false)
    expect(isAppFeature(1)).toBe(false)
  })
})
