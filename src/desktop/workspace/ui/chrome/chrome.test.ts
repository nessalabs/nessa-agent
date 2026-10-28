// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { afterEach, beforeEach, expect, it } from "vitest"

let sheet: HTMLStyleElement
let workspace: HTMLElement
let sidebar: HTMLElement
let bar: HTMLElement
let button: HTMLElement
let icon: Element

beforeEach(() => {
  sheet = document.createElement("style")
  sheet.textContent = ["./chrome.css", "../source-list/source-list.css"]
    .map((path) => readFileSync(new URL(path, import.meta.url), "utf8"))
    .join("\n")
  document.head.append(sheet)
  workspace = document.createElement("main")
  workspace.className = "workspace"
  workspace.dataset.sidebar = "open"
  workspace.innerHTML = `<aside class="workspace-sidebar"><div class="desktop-column-bar"><span class="desktop-column-action"><button class="desktop-icon-button" aria-label="New Session"><svg></svg></button></span></div></aside>`
  document.body.append(workspace)
  sidebar = workspace.querySelector("aside")!
  bar = workspace.querySelector<HTMLElement>(".desktop-column-bar")!
  button = workspace.querySelector("button")!
  icon = workspace.querySelector("svg")!
})
afterEach(() => {
  workspace.remove()
  sheet.remove()
})
const visibility = (element: Element) => getComputedStyle(element).visibility

it("keeps the below-titlebar action painted only while the folding row is held", () => {
  workspace.toggleAttribute("data-spare-held", true)
  expect(visibility(button)).toBe("visible")
  workspace.dataset.sidebar = "closed"
  workspace.toggleAttribute("data-folding", true)
  // In the flat window the column head is below the fixed titlebar.
  // Its painted row follows the same folding hold as the sidebar body.
  expect(visibility(sidebar)).toBe("visible")
  // jsdom does not resolve this custom-property shorthand into geometry.
  // Check the live stylesheet declaration; browser safe-area checks own boxes.
  const sidebarRule = [...(sheet.sheet?.cssRules ?? [])].find(
    (rule) => "selectorText" in rule && rule.selectorText === ".workspace-sidebar",
  ) as CSSStyleRule | undefined
  expect(sidebarRule?.style.getPropertyValue("padding-top")).toMatch(
    /^var\(--desktop-titlebar-height\)/,
  )
  for (const element of [bar, button, icon]) expect(visibility(element)).toBe("visible")
  workspace.removeAttribute("data-spare-held")
  for (const element of [bar, button, icon]) expect(visibility(element)).toBe("hidden")
})

it("keeps the existing peek and reopen paths available", () => {
  workspace.dataset.sidebar = "closed"
  workspace.toggleAttribute("data-spare-held", true)
  workspace.toggleAttribute("data-peek", true)
  // jsdom has no transition clock: this is the peek's computed destination,
  // not a claim that it paints before its existing 360 ms reveal delay.
  expect(visibility(button)).toBe("visible")
  workspace.removeAttribute("data-peek")
  expect(visibility(button)).toBe("hidden")
  workspace.dataset.sidebar = "open"
  expect(visibility(button)).toBe("visible")
})
