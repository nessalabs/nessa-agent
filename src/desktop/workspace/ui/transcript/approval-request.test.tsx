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
import type { ApprovalChoice, ApprovalOption } from "../../model/transcript"
import { ApprovalActions, ApprovalCommand, approvalReason } from "./approval-request"
import { bidiControls } from "./bidi-controls.mjs"
import { spoken } from "./said"

const offered: readonly ApprovalOption[] = [
  { id: "deny", label: "Deny", choice: "deny" },
  { id: "always", label: "Always Allow", choice: "always" },
  { id: "once", label: "Allow Once", choice: "once" },
]

let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

it("isolates an app's names in its reason, and its tool at the start of the command, leaving the message", async () => {
  const server = "evil\u202Egnp.exe"
  const tool = "c\u2069\u202Bd"
  const reason = `The ${tool} app on ${server} asks to send a message as you`
  expect(spoken(approvalReason({ origin: { kind: "app", server, tool }, reason }))).toBe(
    "The \u2068c\uFFFD\uFFFDd\u2069 app on \u2068evil\uFFFDgnp.exe\u2069 asks to send a message as you",
  )
  // A name that contains another is taken whole.
  expect(
    spoken(
      approvalReason({
        origin: { kind: "app", server: "ab", tool: "a" },
        reason: "run ab",
      }),
    ),
  ).toBe("run \u2068ab\u2069")
  expect(
    spoken(approvalReason({ origin: { kind: "agent" }, reason: "Runs the tests." })),
  ).toBe("Runs the tests.")
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <ApprovalCommand command={`${tool} {"text":"keep \u202E this"}`} name={tool} />,
    ),
  )
  expect(host.querySelector("bdi")?.textContent).toBe("c\uFFFD\uFFFDd")
  expect(host.querySelector(".workspace-approval-command")?.textContent).toContain(
    '{"text":"keep \u202E this"}',
  )
  await act(async () => root.unmount())
})

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

const byName = (name: string) =>
  [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (button) => (button.getAttribute("aria-label") ?? button.textContent) === name,
  )

it("offers every answer the review carries, each giving its own", async () => {
  const given: ApprovalChoice[] = []
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <ApprovalActions
        options={offered}
        disabled={false}
        onAnswer={(option) => given.push(option.choice)}
      />,
    ),
  )
  for (const name of ["Deny", "Always Allow", "Allow Once"]) byName(name)?.click()
  expect(given).toEqual(["deny", "always", "once"])
  expect(byName("More Ways to Allow")).toBeDefined()
  await act(async () => root.unmount())
})

it("offers only the answers the review carries, in the review's words (#444)", async () => {
  const given: ApprovalChoice[] = []
  const root = createRoot(host)
  const review: readonly ApprovalOption[] = [
    { id: "opt-b", label: "Nope", choice: "deny" },
    { id: "opt-a", label: "Sure", choice: "once" },
  ]
  await act(async () =>
    root.render(
      <ApprovalActions
        options={review}
        disabled={false}
        onAnswer={(option) => given.push(option.choice)}
      />,
    ),
  )
  expect(byName("Always Allow")).toBeUndefined()
  expect(byName("Allow Once")).toBeUndefined()
  expect(byName("More Ways to Allow")).toBeUndefined()
  expect(byName("Deny")).toBeUndefined()
  for (const name of ["Nope", "Sure"]) byName(name)?.click()
  expect(given).toEqual(["deny", "once"])
  await act(async () => root.unmount())
})

it("gives the clicked option when two answers allow the same way", async () => {
  const given: string[] = []
  const root = createRoot(host)
  const review: readonly ApprovalOption[] = [
    { id: "ship", label: "Ship it", choice: "once" },
    { id: "run", label: "Run it", choice: "once" },
  ]
  await act(async () =>
    root.render(
      <ApprovalActions
        options={review}
        disabled={false}
        onAnswer={(option) => given.push(option.id)}
      />,
    ),
  )
  byName("Run it")?.click()
  expect(given).toEqual(["run"])
  await act(async () => root.unmount())
})

it("arranges the answers by the card's width, and wraps a long label inside its button", () => {
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
  // Extra options continue on the next line; a long label wraps inside its button.
  expect(sheet).toMatch(/\.workspace-approval-actions\s*\{[^}]*flex-wrap:\s*wrap/)
  expect(sheet).toMatch(/\.workspace-approval-allow\s*\{[^}]*flex-wrap:\s*wrap/)
  const answers = sheet.match(
    /\.workspace-approval-actions \.workspace-button\s*\{[^}]*\}/,
  )?.[0]
  expect(answers).toMatch(/max-width:\s*100%/)
  expect(answers).toMatch(/min-width:\s*0/)
  expect(answers).toMatch(/white-space:\s*normal/)
  expect(answers).toMatch(/overflow-wrap:\s*anywhere/)
  // The command's words stay whole; only an answer label may break anywhere.
  const word = sheet.match(/\.workspace-approval-word\s*\{[^}]*\}/)?.[0]
  expect(word).toMatch(/white-space:\s*pre/)
  expect(word).not.toMatch(/overflow-wrap/)
  expect(sheet).toMatch(/text-indent:\s*-2ch/)
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
    root.render(
      <ApprovalActions options={offered} disabled={false} onAnswer={() => {}} />,
    ),
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

it("draws an agent's command in order, with a bidi control visible (#553)", async () => {
  const argument = "\u202Emoc.live@bob\u202C"
  const command = `send ${JSON.stringify({ to: argument })}`
  const root = createRoot(host)
  await act(async () => root.render(<ApprovalCommand command={command} />))
  const block = host.querySelector(".workspace-approval-command")
  const shown = block?.textContent?.replace(/^\$ /, "") ?? ""
  expect(shown).not.toMatch(bidiControls)
  expect(shown.indexOf("moc.live@bob")).toBeGreaterThan(shown.indexOf("\\u202e"))
  expect(block?.querySelector("bdi")).not.toBeNull()
  expect(JSON.parse(shown.slice(shown.indexOf(" ")))).toEqual({ to: argument })
  await act(async () => root.unmount())
})
