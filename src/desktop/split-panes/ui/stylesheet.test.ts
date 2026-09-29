/**
 * What the split panes' stylesheet promises, read from it: nothing carried
 * is painted under the window's controls.
 */
import { readFileSync } from "node:fs"
import { expect, it } from "vitest"
import { classes } from "../adapters/dom/marks"

it("draws everything carried in a layer that begins below the titlebar row and clips there", () => {
  const sheet = readFileSync(new URL("./split-panes.css", import.meta.url), "utf8")
  const body = (selector: string) => sheet.slice(sheet.indexOf(selector)).split("}")[0]
  // Nothing carried is painted under the window's controls, whatever it passes over.
  const layer = body(`.${classes.layer} {`)
  expect(layer).toMatch(/inset:\s*var\(--desktop-titlebar-height\) 0 0 0/)
  expect(layer).toMatch(/overflow:\s*clip/)
  expect(layer).toMatch(/position:\s*fixed/)
  // The carrier inside it is placed back at the window's origin, not fixed past the clip.
  const carrier = body(`.${classes.carrier} {`)
  expect(carrier).toMatch(/position:\s*absolute/)
  expect(carrier).toMatch(/top:\s*calc\(-1 \* var\(--desktop-titlebar-height\)\)/)
})
