import { tsImport } from "tsx/esm/api"

const { settingsCategory, showsTabs, tabsOf } = await tsImport(
  "../../../../src/desktop/settings/model/settings-catalogue.ts",
  import.meta.url,
)

/** Compare the visible strip with the catalogue's own tab visibility and order. */
export function settingsTabFailures(category, shown) {
  const expected = showsTabs(category) ? tabsOf(category).map((tab) => tab.label) : []
  if (JSON.stringify(shown) === JSON.stringify(expected)) return []
  return [
    `${settingsCategory(category).label} shows tabs ${JSON.stringify(shown)}, expected ${JSON.stringify(expected)} from its catalogue`,
  ]
}
