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
