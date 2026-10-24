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
  workspace.dataset.sidebarRow = "open"
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

it("hides a closing sidebar's action immediately while the spare hold keeps its body painted", () => {
  workspace.toggleAttribute("data-spare-held", true)
  expect(visibility(button)).toBe("visible")
  workspace.dataset.sidebar = "closed"
  // A held layout or an in-flight transform cannot authorize this control
  // inside the reserved corner. The body may still be held in the row.
  expect(visibility(sidebar)).toBe("visible")
  for (const element of [bar, button, icon]) expect(visibility(element)).toBe("hidden")
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
