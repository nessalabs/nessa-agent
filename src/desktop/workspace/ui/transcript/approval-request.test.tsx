// @vitest-environment jsdom
/**
 * An approval's command and answers, as every card draws them: the command
 * breaks only between whole words, and every answer is there to arrange —
 * the stylesheet's container queries choose which show at the card's width
 * (checked frame by frame in a browser; here, that the parts and the rules
 * they need exist).
 */
import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { ApprovalActions, ApprovalCommand, type ApprovalChoice } from "./approval-request"

let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

it("keeps each word of the command whole, breaking only at its spaces", async () => {
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <ApprovalCommand command="cargo run -p nessa-gateway --  --simulate-clients 200" />,
    ),
  )
  const words = [...host.querySelectorAll(".workspace-approval-word")].map(
    (word) => word.textContent,
  )
  expect(words).toEqual([
    "cargo",
    "run",
    "-p",
    "nessa-gateway",
    "--",
    "--simulate-clients",
    "200",
  ])
  // The command reads exactly as written, spaces and all; the prompt is not read.
  const block = host.querySelector(".workspace-approval-command")
  expect(block?.textContent).toBe(
    "$ cargo run -p nessa-gateway --  --simulate-clients 200",
  )
  expect(
    host.querySelector(".workspace-approval-prompt")?.getAttribute("aria-hidden"),
  ).toBe("true")
  await act(async () => root.unmount())
})

it("offers every answer, each giving its own", async () => {
  const given: ApprovalChoice[] = []
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <ApprovalActions disabled={false} onAnswer={(choice) => given.push(choice)} />,
    ),
  )
  const byName = (name: string) =>
    [...host.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => (button.getAttribute("aria-label") ?? button.textContent) === name,
    )
  for (const name of ["Deny", "Always Allow", "Allow Once"]) byName(name)?.click()
  expect(given).toEqual(["deny", "always", "once"])
  expect(byName("More Ways to Allow")).toBeDefined()
  await act(async () => root.unmount())
})

it("arranges the answers by the card's width, with no rule that leaves one alone on a row", () => {
  // Read from the repository root: under jsdom, this module's URL is not a file's.
  const sheet = readFileSync(
    resolve("src/desktop/workspace/ui/transcript/approval-card.css"),
    "utf8",
  )
  expect(sheet).toMatch(/container:\s*approval-answers\s*\/\s*inline-size/)
  // Medium: one row at the right, "Always". Narrow: a column, Allow Once first.
  expect(sheet).toContain("@container approval-answers (width < 380px)")
  expect(sheet).toContain("@container approval-answers (width < 280px)")
  const narrow = sheet.slice(sheet.indexOf("@container approval-answers (width < 280px)"))
  expect(narrow).toMatch(/flex-direction:\s*column/)
  expect(narrow).toMatch(/\.workspace-approval-always\s*\{\s*display:\s*none/)
  // Nothing wraps: a wrapped row is where a button was left alone.
  expect(sheet).not.toMatch(
    /\.workspace-approval-(?:actions|allow)\s*\{[^}]*flex-wrap:\s*wrap/,
  )
  // Words are never broken; lines after the first hang clear of the `$`.
  expect(sheet).toMatch(/\.workspace-approval-word\s*\{\s*white-space:\s*pre/)
  expect(sheet).toMatch(/text-indent:\s*-2ch/)
  expect(sheet).not.toMatch(/overflow-wrap:\s*anywhere/)
})

it("takes the keyboard where a word is wider than the card, so the arrows scroll it", async () => {
  let report: () => void = () => {}
  class Observer {
    constructor(callback: () => void) {
      report = callback
    }
    observe() {}
    disconnect() {}
  }
  const was = globalThis.ResizeObserver
  Object.assign(globalThis, { ResizeObserver: Observer })
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <ApprovalCommand command="xcrun notarytool submit Nessa_0.9.0_aarch64.dmg" />,
    ),
  )
  const block = host.querySelector<HTMLElement>(".workspace-approval-command")
  if (!block) throw new Error("no command")
  Object.defineProperty(block, "clientWidth", { configurable: true, value: 78 })
  Object.defineProperty(block, "scrollWidth", { configurable: true, value: 202 })
  await act(async () => report())
  expect(block.hasAttribute("data-overflow")).toBe(true)
  expect(block.tabIndex).toBe(0)
  const scrolled: number[] = []
  block.scrollBy = ((options: ScrollToOptions) =>
    scrolled.push(options.left ?? 0)) as never
  await act(async () => {
    block.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
    )
    block.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }))
  })
  expect(scrolled).toEqual([48, -48])
  Object.defineProperty(block, "scrollWidth", { configurable: true, value: 78 })
  await act(async () => report())
  expect(block.hasAttribute("tabindex")).toBe(false)
  await act(async () => root.unmount())
  Object.assign(globalThis, { ResizeObserver: was })
})

it("presses an answer once for a held key: its repeats press nothing", async () => {
  const root = createRoot(host)
  await act(async () =>
    root.render(<ApprovalActions disabled={false} onAnswer={() => {}} />),
  )
  const allow = host.querySelector<HTMLButtonElement>(".workspace-approval-once")
  if (!allow) throw new Error("no Allow Once")
  const key = (repeat: boolean) => {
    const event = new KeyboardEvent("keydown", {
      key: "Enter",
      code: "Enter",
      repeat,
      bubbles: true,
      cancelable: true,
    })
    allow.dispatchEvent(event)
    return event.defaultPrevented
  }
  // The first press presses it, as the browser does; each repeat is stopped.
  expect(key(false)).toBe(false)
  expect(key(true)).toBe(true)
  await act(async () => root.unmount())
})
