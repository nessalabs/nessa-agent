import { describe, expect, it } from "vitest"
import {
  dekOf,
  firstTabOf,
  namesItsTab,
  searchSettings,
  setting,
  settingsCategories,
  settingsCategory,
  settingsEntries,
  settingsTab,
  showsTabs,
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

  it("keeps experiments under Advanced, just before About, and not under General", () => {
    const ids = settingsCategories.map((category) => category.id)
    expect(ids.slice(-2)).toEqual(["advanced", "about"])
    expect(tabsOf("advanced").map((tab) => tab.id)).toEqual(["experimental"])
    expect(settingsTab("experimental")).toMatchObject({
      category: "advanced",
      label: "Experimental",
    })
    expect(tabsOf("general").map((tab) => tab.id)).not.toContain("experimental")
    // The previews on offer: the side rail, and nothing else.
    expect(
      settingsEntries
        .filter((entry) => settingsTab(entry.tab).category === "advanced")
        .map((entry) => entry.id),
    ).toEqual(["side-rail"])
  })

  it("shows a category's tabs only when it has several to choose from", () => {
    expect(showsTabs("general")).toBe(true)
    expect(showsTabs("advanced")).toBe(false)
    expect(showsTabs("about")).toBe(false)
  })

  it("never names a tab in the strip as its category is named, so no word shows twice at once", () => {
    for (const category of settingsCategories)
      if (showsTabs(category.id))
        for (const tab of tabsOf(category.id))
          expect(tab.label, category.id).not.toBe(category.label)
  })

  it("gives a category a line under its title only where its name does not say enough", () => {
    expect(dekOf("advanced")).toBe("Early features you can try before they are finished.")
    expect(dekOf("appearance")).toBeUndefined()
    // One short line: never more than a glance.
    for (const category of settingsCategories)
      expect((dekOf(category.id) ?? "").length, category.id).toBeLessThanOrEqual(64)
  })

  it("knows a setting named like its tab, whose group needs no title on screen", () => {
    expect(namesItsTab("agents")).toBe(true)
    expect(namesItsTab("workspace-layout")).toBe(true)
    expect(namesItsTab("mcp-servers")).toBe(false)
    expect(namesItsTab("icon-family")).toBe(false)
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
    expect(
      searchSettings("fingerprint").some((match) => match.setting === "linked-devices"),
    ).toBe(true)
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

  it("finds Advanced › Experimental by its name and by what people call it", () => {
    expect(searchSettings("advanced")[0]).toEqual({
      category: "advanced",
      tab: "experimental",
      label: "Advanced",
      trail: "",
    })
    for (const query of ["experimental", "labs", "preview"])
      expect(searchSettings(query)[0], query).toEqual({
        category: "advanced",
        tab: "experimental",
        label: "Experimental",
        trail: "Advanced",
      })
    // With no tab strip on its page, a setting there is placed by its category alone.
    expect(showsTabs("advanced")).toBe(false)
  })

  it("treats search syntax as plain text", () => {
    expect(() => searchSettings("(.*")).not.toThrow()
    expect(searchSettings("⌘-click")[0]?.setting).toBe("cmd-click-beside")
  })
})
