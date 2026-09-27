import { describe, expect, it } from "vitest"
import {
  firstTabOf,
  searchSettings,
  setting,
  settingsCategories,
  settingsCategory,
  settingsEntries,
  settingsTab,
  tabsOf,
} from "./settings-catalogue"

describe("the settings catalogue", () => {
  it("gives every tab an id no other tab has, so a tab names its category", () => {
    const ids = settingsCategories.flatMap((category) =>
      category.tabs.map((tab) => tab.id),
    )
    expect(new Set(ids).size).toBe(ids.length)
  })

  it("gives every category and setting a unique id", () => {
    const categories = settingsCategories.map((category) => category.id)
    expect(new Set(categories).size).toBe(categories.length)
    const settings = settingsEntries.map((entry) => entry.id)
    expect(new Set(settings).size).toBe(settings.length)
  })

  it("keeps the sidebar short", () => {
    expect(settingsCategories.length).toBeLessThanOrEqual(8)
  })

  it("puts every tab under the category that lists it", () => {
    for (const category of settingsCategories) {
      expect(tabsOf(category.id).map((tab) => tab.id)).toEqual(
        category.tabs.map((tab) => tab.id),
      )
      for (const tab of category.tabs)
        expect(settingsTab(tab.id).category).toBe(category.id)
    }
  })

  it("opens a category on its first tab", () => {
    expect(firstTabOf("appearance")).toBe("theme")
    expect(firstTabOf("about")).toBe("about")
  })

  it("reads a setting's own name and description", () => {
    expect(setting("tint-from-picture")).toMatchObject({
      tab: "header",
      label: "Tint app from header picture",
    })
  })

  it("names every category by its label", () => {
    expect(settingsCategory("privacy").label).toBe("Privacy & Permissions")
  })
})

describe("searchSettings", () => {
  it("finds nothing for an empty or blank query", () => {
    expect(searchSettings("")).toEqual([])
    expect(searchSettings("   ")).toEqual([])
  })

  it("jumps to the tab that holds a setting", () => {
    const [first] = searchSettings("tint")
    expect(first).toEqual({
      category: "appearance",
      tab: "header",
      setting: "tint-from-picture",
      label: "Tint app from header picture",
      trail: "Appearance › Header",
    })
  })

  it("finds a tab by its label", () => {
    expect(searchSettings("notifications")[0]).toEqual({
      category: "general",
      tab: "notifications",
      label: "Notifications",
      trail: "General",
    })
  })

  it("finds a category by its label, on its first tab", () => {
    expect(searchSettings("connections")[0]).toMatchObject({
      category: "connections",
      tab: "agents",
      label: "Connections",
    })
  })

  it("finds a setting through its keywords", () => {
    expect(searchSettings("lucide").map((match) => match.setting)).toEqual([
      "icon-family",
    ])
  })

  it("ignores case and needs every word somewhere", () => {
    expect(searchSettings("SESSION LIST")[0]?.setting).toBe("show-session-list")
    expect(searchSettings("session zebra")).toEqual([])
  })

  it("ranks a label that starts with the query above one that only contains it", () => {
    const labels = searchSettings("session").map((match) => match.label)
    expect(labels.indexOf("Sessions")).toBeLessThan(labels.indexOf("Show session list"))
  })

  it("does not return every setting in a category for the category's name", () => {
    const matches = searchSettings("appearance")
    expect(matches).toHaveLength(1)
    expect(matches[0]?.setting).toBeUndefined()
  })

  it("does not repeat a category whose first tab shares its name", () => {
    expect(searchSettings("general").filter((m) => m.label === "General")).toHaveLength(1)
  })

  it("names a single-tab category alone in the trail", () => {
    expect(searchSettings("version")[0]).toMatchObject({ tab: "about", trail: "About" })
  })

  it("treats search syntax as plain text", () => {
    expect(() => searchSettings("(.*")).not.toThrow()
    expect(searchSettings("⌘-click")[0]?.setting).toBe("cmd-click-beside")
  })
})
