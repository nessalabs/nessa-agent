import { describe, expect, it } from "vitest"
import { widgetItem, widgetOfItem } from "./pane-item"

describe("a pane item", () => {
  it("names a widget, and reads back as that widget", () => {
    const item = widgetItem({ plugin: "experiment", id: "checkout:v2" })
    expect(item).toBe("widget:experiment:checkout:v2")
    expect(widgetOfItem(item)).toEqual({ plugin: "experiment", id: "checkout:v2" })
  })

  it("is a session when it is not a whole widget name", () => {
    expect(widgetOfItem("retry-budget")).toBeNull()
    expect(widgetOfItem("widget:")).toBeNull()
    expect(widgetOfItem("widget:experiment")).toBeNull()
    expect(widgetOfItem("widget:experiment:")).toBeNull()
    expect(widgetOfItem("widget::id")).toBeNull()
  })
})
