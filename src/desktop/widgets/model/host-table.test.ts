/** ADR 326's table, row by row and place by place. */
import { describe, expect, it } from "vitest"
import {
  hostDraws,
  hostLines,
  offersWindow,
  widgetOrigin,
  widgetTitle,
} from "./host-table"
import type { WidgetAnswer, WidgetState } from "./widget-state"

const answer = (state: WidgetState): WidgetAnswer => ({
  registered: true,
  name: "Experiment",
  state,
})
const ready = answer({ kind: "ready", title: "Checkout climb", origin: "a" })
const everywhere = { inline: true, window: true }
const paneOnly = { inline: false, window: false }

describe("ready, its plugin registered", () => {
  it("draws the plugin's view in each place it offers", () => {
    expect(hostDraws("inline", ready, everywhere)).toEqual({ kind: "view" })
    expect(hostDraws("pane", ready, everywhere)).toEqual({ kind: "view" })
    expect(hostDraws("window", ready, everywhere)).toEqual({ kind: "view" })
  })

  it("draws a row with its title and Open inline, with no inline view", () => {
    expect(hostDraws("inline", ready, paneOnly)).toEqual({
      kind: "row",
      title: "Checkout climb",
    })
  })

  it("draws a pane always, and says it cannot show a window its plugin does not offer", () => {
    expect(hostDraws("pane", ready, paneOnly)).toEqual({ kind: "view" })
    expect(hostDraws("window", ready, paneOnly)).toEqual({
      kind: "line",
      text: hostLines.unshowable,
      closes: true,
    })
  })
})

describe("not ready, or no plugin", () => {
  const rows: [string, WidgetAnswer, string | null][] = [
    ["unread", answer({ kind: "unread" }), null],
    ["missing", answer({ kind: "missing" }), "This is no longer available"],
    ["off", answer({ kind: "off" }), "Turned off in Settings › Advanced › Experimental"],
    ["unshowable", answer({ kind: "unshowable" }), "Can't show this here"],
    ["unregistered", { registered: false }, "Can't show this here"],
  ]
  for (const [row, given, line] of rows)
    it(`draws ${row} as ADR 326 says, in each place`, () => {
      for (const place of ["inline", "pane", "window"] as const) {
        const drawn = hostDraws(place, given, everywhere)
        if (line === null) expect(drawn).toEqual({ kind: "waiting", name: "Experiment" })
        else
          expect(drawn).toEqual({ kind: "line", text: line, closes: place !== "inline" })
      }
    })
})

describe("what the chrome says", () => {
  it("names a widget by its title once read, else by its plugin's name", () => {
    expect(widgetTitle(ready)).toBe("Checkout climb")
    expect(widgetTitle(answer({ kind: "unread" }))).toBe("Experiment")
    expect(widgetTitle(answer({ kind: "missing" }))).toBe("Experiment")
    expect(widgetTitle({ registered: false })).toBe("Unavailable")
  })

  it("leads back to the origin of a ready widget only", () => {
    expect(widgetOrigin(ready)).toBe("a")
    expect(widgetOrigin(answer({ kind: "ready", title: "No origin" }))).toBeUndefined()
    expect(widgetOrigin(answer({ kind: "unread" }))).toBeUndefined()
    expect(widgetOrigin({ registered: false })).toBeUndefined()
  })

  it("offers the window for a ready widget whose plugin draws one", () => {
    expect(offersWindow(ready, everywhere)).toBe(true)
    expect(offersWindow(ready, paneOnly)).toBe(false)
    expect(offersWindow(answer({ kind: "unread" }), everywhere)).toBe(false)
    expect(offersWindow({ registered: false }, everywhere)).toBe(false)
  })
})
