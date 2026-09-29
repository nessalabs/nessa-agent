import { expect, it } from "vitest"
import { settingsSidebar, settingsSidebarFits } from "./settings-sidebar"

it("keeps the sidebar while a page still has its room beside it, to the pixel", () => {
  const edge = settingsSidebar.width + settingsSidebar.minContent
  expect(settingsSidebarFits(edge)).toBe(true)
  expect(settingsSidebarFits(edge - 1)).toBe(false)
  expect(settingsSidebarFits(550)).toBe(false)
  expect(settingsSidebarFits(1440)).toBe(true)
})
