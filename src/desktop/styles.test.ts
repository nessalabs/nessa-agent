/**
 * What the stylesheet promises about motion, read from the stylesheet: the
 * glide a first message's composer lands on never passes its place.
 */
import { readFileSync } from "node:fs"
import { expect, it } from "vitest"

const styles = readFileSync(new URL("./styles.css", import.meta.url), "utf8")

/**
 * A pane in the window's corner — or where a drag's preview would put it
 * there (`data-drag-corner`, `split-panes/adapters/dom/drag.ts`), and not
 * where it would leave — as the stylesheets select it.
 */
const cornered = `.workspace[data-panes-alone]
  .workspace-pane:is([data-corner]:not([data-drag-corner="no"]), [data-drag-corner="yes"])
`

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
      `${cornered}  .workspace-pane-header {`,
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

it("keeps the list's inline title out from under the window's controls while a drop would fold the sidebar", () => {
  // The fold's preview slides the list to the window's edge, under the
  // controls; the title the fold moves to its own row fades from this one.
  const sheet = readFileSync(
    new URL("./workspace/ui/session-list/session-list.css", import.meta.url),
    "utf8",
  )
  const selector = '.workspace[data-drag-takes-spare][data-sidebar="open"]'
  const body = sheet.slice(sheet.indexOf(selector)).split("}")[0]
  expect(body).toContain(".desktop-column-title")
  expect(body).toMatch(/opacity:\s*0;/)
})

it("names what a flight restyles, so beginning one restyles a few elements, not every one in every pane", () => {
  // `.workspace[data-flipping] .workspace-pane > *` made the browser restyle all
  // ~1,100 elements of four panes as a flight began: a rule whose subject is any
  // element cannot be aimed. Named, it restyles two per pane.
  const sheet = ["./workspace/ui/panes/panes.css", "./split-panes/ui/split-panes.css"]
    .map((path) => readFileSync(new URL(path, import.meta.url), "utf8"))
    .join("\n")
  const flightRules =
    sheet.match(/^[^{}/]*\[data-(?:flipping|drag-reflow)\][^{]*\{/gm) ?? []
  expect(flightRules.length).toBeGreaterThan(0)
  for (const rule of flightRules) expect(rule, rule).not.toMatch(/(?:>|\s)\*\s*(?:,|\{)/)
})

it("keeps the list and the panes under the Agents overview laid out, only unseen", () => {
  // A pane command asked while the overview is open measures the panes' room:
  // it must be the room they have, so nothing under the overview may leave the
  // layout (display, content-visibility) or change its size.
  const sheet = readFileSync(
    new URL("./workspace/ui/layouts/layouts.css", import.meta.url),
    "utf8",
  )
  const selector =
    '.workspace[data-content="agents"] .workspace-list,\n.workspace[data-content="agents"] .workspace-list-edge,\n.workspace[data-content="agents"] .workspace-chat {'
  const body = sheet.slice(sheet.indexOf(selector)).split("}")[0]
  expect(sheet).toContain(selector)
  expect(body).toMatch(/visibility:\s*hidden/)
  const everyAgentsRule = sheet.match(/\[data-content="agents"\][^{]*\{[^}]*\}/g) ?? []
  for (const rule of everyAgentsRule)
    expect(rule, rule).not.toMatch(/display:|content-visibility|width:|padding/)
  // The overview's own sheet sets nothing on what is under it.
  const overview = readFileSync(
    new URL("./workspace/ui/overview/overview.css", import.meta.url),
    "utf8",
  )
  expect(overview).not.toMatch(/\.workspace-(?:list|chat)\b/)
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

it("keeps a running step's time on one line, whatever the pane's width", () => {
  const sheet = readFileSync(
    new URL("./workspace/ui/transcript/transcript.css", import.meta.url),
    "utf8",
  )
  const body = sheet.slice(sheet.indexOf(".workspace-live-time {")).split("}")[0]
  expect(body).toMatch(/white-space:\s*nowrap/)
  expect(body).toMatch(/flex-shrink:\s*0/)
})

it("paints nothing of a pane under the window's controls: its content below the titlebar row, its picture band after them", () => {
  const panes = readFileSync(
    new URL("./workspace/ui/panes/panes.css", import.meta.url),
    "utf8",
  )
  const body = (sheet: string, selector: string) =>
    sheet.slice(sheet.indexOf(selector)).split("}")[0]
  // The header is the titlebar row; what follows it starts below the titlebar's height.
  expect(body(panes, ".workspace-pane-header {")).toMatch(
    /margin-bottom:\s*calc\(\s*var\(--desktop-titlebar-height\) - var\(--desktop-gutter\)/,
  )
  // In the corner, the band begins after the controls.
  const corner = `${cornered}  .desktop-header[data-sliver] {`
  expect(styles).toContain(corner)
  expect(body(styles, corner)).toMatch(
    /left:\s*calc\(var\(--desktop-titlebar-safe-start\) - var\(--desktop-gutter\)\)/,
  )
  // While panes travel, it waits out of sight.
  expect(
    body(
      panes,
      ".workspace:is([data-flipping], [data-drag-reflow]) .desktop-header[data-sliver] {",
    ),
  ).toMatch(/opacity:\s*0/)
})

it("moves Settings' sidebar by transform, never by animating its width", () => {
  const sheet = readFileSync(
    new URL("./settings/ui/settings.css", import.meta.url),
    "utf8",
  )
  const body = sheet.slice(sheet.indexOf(".settings-sidebar {")).split("}")[0]
  expect(body).not.toMatch(/transition/)
})

it("draws everything carried in a layer that begins below the titlebar row and clips there", () => {
  const sheet = readFileSync(
    new URL("./split-panes/ui/split-panes.css", import.meta.url),
    "utf8",
  )
  const body = (selector: string) => sheet.slice(sheet.indexOf(selector)).split("}")[0]
  // Nothing carried is painted under the window's controls, whatever it passes over.
  const layer = body(".split-panes-layer {")
  expect(layer).toMatch(/inset:\s*var\(--desktop-titlebar-height\) 0 0 0/)
  expect(layer).toMatch(/overflow:\s*clip/)
  expect(layer).toMatch(/position:\s*fixed/)
  // The carrier inside it is placed back at the window's origin, not fixed past the clip.
  const carrier = body(".split-panes-carrier {")
  expect(carrier).toMatch(/position:\s*absolute/)
  expect(carrier).toMatch(/top:\s*calc\(-1 \* var\(--desktop-titlebar-height\)\)/)
})
