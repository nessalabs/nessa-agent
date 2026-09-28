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
