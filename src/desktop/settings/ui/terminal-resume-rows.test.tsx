// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { TerminalResumeSettings, TerminalResumeState } from "@nessa/client"
import type { TerminalResumeGateway } from "../adapters/terminal-resume-gateway"
import { RuleRefused } from "../model/terminal-resume"
import { TerminalResumeProvider, TerminalResumeRows } from "./terminal-resume-rows"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(() => {
  act(() => root.unmount())
  container.remove()
})

/** A gateway that keeps what it is given, as the real one does. */
function keeping(start: TerminalResumeState) {
  let kept = start
  const gateway = {
    read: vi.fn(async () => kept),
    set: vi.fn(async (settings: TerminalResumeSettings) => {
      // As the gateway refuses a path that does not start with /.
      if (
        settings.rules.some(
          (rule) => rule.program.includes("/") && !rule.program.startsWith("/"),
        )
      )
        throw new RuleRefused("program")
      kept = {
        settings,
        saved: settings.enabled
          ? kept.saved.map((saved) => ({
              ...saved,
              comesBack: settings.rules.some((rule) => rule.program === "nvim"),
            }))
          : [],
      }
      return kept
    }),
    forget: vi.fn(async (terminalId: string) => {
      kept = {
        ...kept,
        saved: kept.saved.filter((saved) => saved.terminalId !== terminalId),
      }
      return kept
    }),
  } satisfies TerminalResumeGateway
  return gateway
}

const settle = () => act(async () => {})

async function draw(gateway: TerminalResumeGateway | undefined) {
  act(() =>
    root.render(
      <TerminalResumeProvider gateway={gateway}>
        <TerminalResumeRows />
      </TerminalResumeProvider>,
    ),
  )
  await settle()
}

const toggle = () => container.querySelector<HTMLButtonElement>('[role="switch"]')
const field = (name: string) =>
  container.querySelector<HTMLInputElement>(
    `[data-terminal-resume-${name}]`,
  ) as HTMLInputElement
const type = (input: HTMLInputElement, text: string) =>
  act(() => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set
    set?.call(input, text)
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
const button = (text: string) =>
  [...container.querySelectorAll("button")].find((each) => each.textContent === text)

const off: TerminalResumeState = {
  settings: { enabled: false, rules: [] },
  saved: [],
}

it("is off until the person turns it on, and says nothing comes back without a rule", async () => {
  const gateway = keeping(off)
  await draw(gateway)
  expect(toggle()?.getAttribute("aria-checked")).toBe("false")
  act(() => toggle()?.click())
  await settle()
  expect(gateway.set).toHaveBeenCalledWith({ enabled: true, rules: [] })
  expect(toggle()?.getAttribute("aria-checked")).toBe("true")
  expect(container.textContent).toContain("Nothing comes back until you add one.")
})

it("adds a program rule with its folder and arguments, and removes it", async () => {
  const gateway = keeping({ ...off, settings: { enabled: true, rules: [] } })
  await draw(gateway)
  type(field("program"), "codex")
  type(field("folder"), "/work")
  type(field("args"), "resume --last")
  act(() => button("Add")?.click())
  await settle()
  expect(gateway.set).toHaveBeenLastCalledWith({
    enabled: true,
    rules: [
      { program: "codex", action: "allow", folder: "/work", args: ["resume", "--last"] },
    ],
  })
  const rule = container.querySelector('[data-terminal-resume-rule="codex"]')
  expect(rule?.textContent).toContain("Allow · in /work · with resume --last")
  expect(field("program").value).toBe("")
  act(() => button("Remove")?.click())
  await settle()
  expect(gateway.set).toHaveBeenLastCalledWith({ enabled: true, rules: [] })
  expect(container.querySelector("[data-terminal-resume-rule]")).toBeNull()
})

it("says why the gateway refused a rule and keeps it to fix", async () => {
  const gateway = keeping({ ...off, settings: { enabled: true, rules: [] } })
  await draw(gateway)
  type(field("program"), "bin/claude")
  act(() => button("Add")?.click())
  await settle()
  expect(gateway.set).toHaveBeenCalledTimes(1)
  expect(container.querySelector('[role="alert"]')?.textContent).toContain(
    "starting with /",
  )
  expect(field("program").value).toBe("bin/claude")
  expect(container.querySelector("[data-terminal-resume-rule]")).toBeNull()
})

it("lists kept programs, whether each comes back, and forgets one", async () => {
  const gateway = keeping({
    settings: { enabled: true, rules: [{ program: "nvim", action: "allow" }] },
    saved: [
      {
        terminalId: "t_1",
        title: "zsh",
        command: "nvim -S /tmp/s.vim",
        savedAtMs: 1,
        comesBack: true,
        open: true,
      },
      {
        terminalId: "t_2",
        title: "work",
        command: "lazygit",
        savedAtMs: 2,
        comesBack: false,
        open: true,
      },
    ],
  })
  await draw(gateway)
  const kept = container.querySelector('[data-terminal-resume-kept="t_1"]')
  expect(kept?.getAttribute("data-terminal-resume-comes-back")).toBe("true")
  expect(kept?.textContent).toContain("nvim -S /tmp/s.vim · Comes back after a restart")
  expect(
    container
      .querySelector('[data-terminal-resume-kept="t_2"]')
      ?.getAttribute("data-terminal-resume-comes-back"),
  ).toBe("false")
  act(() => button("Forget")?.click())
  await settle()
  expect(gateway.forget).toHaveBeenCalledWith("t_1")
  expect(container.querySelector('[data-terminal-resume-kept="t_1"]')).toBeNull()
})

it("rests with no gateway", async () => {
  await draw(undefined)
  expect(container.textContent).toContain("Connect a gateway")
  expect(toggle()?.hasAttribute("disabled")).toBe(true)
})
