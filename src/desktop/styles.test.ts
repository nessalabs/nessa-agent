/**
 * What the stylesheet promises about motion, read from the stylesheet: the
 * glide a first message's composer lands on never passes its place.
 */
import { readFileSync } from "node:fs"
import { expect, it } from "vitest"

const styles = readFileSync(new URL("./styles.css", import.meta.url), "utf8")

it("glides without overshoot: the composer lands, and never passes its place", () => {
  const curve = /--desktop-glide:\s*linear\(([^)]*)\)/.exec(styles)?.[1]
  expect(curve).toBeDefined()
  const points = (curve ?? "").split(",").map((value) => Number.parseFloat(value))
  expect(points.at(0)).toBe(0)
  expect(points.at(-1)).toBe(1)
  // At most a pixel past its place over a travel of 600px.
  expect(Math.max(...points)).toBeLessThanOrEqual(1 + 1 / 600)
  expect(points.every((point, index) => index === 0 || point >= points[index - 1])).toBe(
    true,
  )
})

it("keeps the surfaces' element resets weightless, so no component's own colour loses to them", () => {
  // `.workspace button { color: inherit }` once outranked `.desktop-send`'s own colour and
  // drew the send arrow in the colour of its fill. A reset under a surface is `:where(...)`.
  const sheets = [
    "./styles.css",
    "./workspace/ui/chrome/chrome.css",
    "./settings/ui/settings.css",
  ].map((path) => readFileSync(new URL(path, import.meta.url), "utf8"))
  const weighty =
    /^[ \t]*\.(?:workspace|settings)\s+(?:button|input|textarea|a|:focus-visible)\b[^{]*\{/gm
  expect(sheets.flatMap((sheet) => sheet.match(weighty) ?? [])).toEqual([])
})

it("starts titlebar-row content at the one safe area, never at a length of its own", () => {
  // The safe area is defined once, from the host's inset and the controls' cluster.
  expect(styles).toMatch(
    /--desktop-titlebar-safe-start:\s*calc\(\s*var\(--desktop-window-controls-inset\)/,
  )
  const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8")
  const rules = [
    // The titlebar's cluster starts after the host's own controls.
    [
      read("./workspace/ui/chrome/chrome.css"),
      ".workspace-titlebar {",
      "var(--desktop-window-controls-inset)",
    ],
    [
      read("./settings/ui/settings.css"),
      ".settings-titlebar {",
      "var(--desktop-window-controls-inset)",
    ],
    // A pane in the window's corner starts its header at the safe area.
    [
      read("./workspace/ui/panes/panes.css"),
      ".workspace[data-panes-alone] .workspace-pane[data-corner] .workspace-pane-header {",
      "var(--desktop-titlebar-safe-start)",
    ],
  ] as const
  for (const [sheet, selector, start] of rules) {
    const body = sheet.slice(sheet.indexOf(selector)).split("}")[0]
    expect(body, selector).toContain(`padding-left: ${start}`)
  }
})

it("starts an inline column title after the window's controls wherever they stand over its column", () => {
  // With the sidebar away, the session list and Settings' page begin at the
  // window's edge, under the controls: an inline title (`ui/column-header.tsx`)
  // starts at the safe area there, never at a length of its own.
  const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8")
  const rules = [
    [
      read("./workspace/ui/session-list/session-list.css"),
      '.workspace[data-sidebar="closed"] .workspace-list .desktop-column-bar {',
    ],
    [
      read("./settings/ui/settings.css"),
      '.settings[data-sidebar="closed"] .settings-content > .desktop-column-bar {',
    ],
    // The sidebar's own action, in the row the controls stand over.
    [
      read("./workspace/ui/source-list/source-list.css"),
      ".workspace-sidebar > .desktop-column-bar {",
    ],
  ] as const
  for (const [sheet, selector] of rules) {
    expect(sheet, selector).toContain(selector)
    const body = sheet.slice(sheet.indexOf(selector)).split("}")[0]
    expect(body, selector).toMatch(
      /padding-left:[^;]*var\(--desktop-titlebar-safe-start\)/,
    )
  }
})

it("names what a flight restyles, so beginning one restyles a few elements, not every one in every pane", () => {
  // `.workspace[data-flipping] .workspace-pane > *` made the browser restyle all
  // ~1,100 elements of four panes as a flight began: a rule whose subject is any
  // element cannot be aimed. Named, it restyles two per pane.
  const sheet = readFileSync(
    new URL("./workspace/ui/panes/panes.css", import.meta.url),
    "utf8",
  )
  const flightRules =
    sheet.match(/^[^{}/]*\[data-(?:flipping|drag-reflow)\][^{]*\{/gm) ?? []
  expect(flightRules.length).toBeGreaterThan(0)
  for (const rule of flightRules) expect(rule, rule).not.toMatch(/(?:>|\s)\*\s*(?:,|\{)/)
})

it("keeps the panes under the Agents overview out of layout and paint", () => {
  // Opening the overview must not lay four conversations out again at a width no one sees.
  const sheet = readFileSync(
    new URL("./experiments/agents-overview/ui/agents-overview.css", import.meta.url),
    "utf8",
  )
  const selector = ".agents-overview-area[data-shown] .workspace-chat {"
  const body = sheet.slice(sheet.indexOf(selector)).split("}")[0]
  expect(sheet).toContain(selector)
  expect(body).toMatch(/content-visibility:\s*hidden/)
})

it("keeps a conversation's sliver of the header still, beneath the text, and out of short panes", () => {
  const body = (selector: string) => styles.slice(styles.indexOf(selector)).split("}")[0]
  expect(styles).toContain(".desktop-header[data-sliver] {")
  // Beneath every word in the pane, and taking no pointer.
  expect(body(".desktop-header[data-sliver] {")).toMatch(/z-index:\s*-1/)
  expect(body(".desktop-header[data-sliver] {")).toMatch(/pointer-events:\s*none/)
  // The night scene's rain and steam hold still in it: no per-frame work.
  expect(styles).toMatch(
    /\.desktop-night-scene\[data-still\] \.desktop-night-scene-rain > span,\s*\.desktop-night-scene\[data-still\] \.desktop-night-scene-steam > span \{\s*animation:\s*none/,
  )
  // Gone from a pane shorter than 220px.
  expect(styles).toMatch(
    /@container workspace-pane \(max-height: 219px\) \{\s*\.desktop-header\[data-sliver\] \{\s*display:\s*none/,
  )
})
