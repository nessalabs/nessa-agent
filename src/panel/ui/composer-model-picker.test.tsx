// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { ModelCatalog } from "../../conversation"
import { ComposerModelPicker } from "./composer-model-picker"

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
  document.body.innerHTML = ""
})

const model = (modelId: string, displayName: string, tokens: number) => ({
  modelId,
  displayName,
  maxContextWindowTokens: tokens,
  reasoning: true,
  imageInput: true,
})

const catalog: ModelCatalog = {
  defaultAgent: "claude",
  agents: [
    {
      agent: "claude",
      defaultModel: "claude-sonnet-5",
      approvalModes: ["ask"],
      models: [
        model("claude-opus-5", "Opus 5", 1_000_000),
        model("claude-sonnet-5", "Sonnet 5", 1_000_000),
      ],
    },
    {
      agent: "codex",
      defaultModel: "gpt-5.6-terra",
      approvalModes: ["ask"],
      models: [model("gpt-5.6-terra", "GPT-5.6 Terra", 1_050_000)],
    },
    {
      agent: "nova",
      defaultModel: "nova-1",
      approvalModes: ["ask"],
      models: [model("nova-1", "Nova One", 512_000)],
    },
  ],
}

function render(onChoose = vi.fn()) {
  act(() => {
    root.render(
      <ComposerModelPicker
        catalog={catalog}
        value={{ agent: "claude", model: "claude-sonnet-5" }}
        onChoose={onChoose}
      />,
    )
  })
  return onChoose
}

function trigger() {
  const button = container.querySelector<HTMLButtonElement>(
    '[data-slot="model-picker-trigger"]',
  )
  if (!button) throw new Error("no trigger")
  return button
}

function open() {
  act(() => {
    trigger().dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 }))
    trigger().click()
  })
}

it("shows only the agent's mark, and names the model for a tooltip and a reader", () => {
  render()

  expect(trigger().getAttribute("aria-label")).toBe("Model: Sonnet 5")
  const hint = container.querySelector('[data-slot="model-hint"]')
  expect(hint?.textContent).toBe("Sonnet 5")
  expect(hint?.getAttribute("aria-hidden")).toBe("true")
  // Not the native tooltip, which waits before it shows.
  expect(container.querySelector("[title]")).toBeNull()
  expect(trigger().querySelector("svg path")).not.toBeNull()
})

it("offers a tab per agent the gateway runs, naming an unlisted one by its id", () => {
  render()
  open()

  const tabs = [...document.body.querySelectorAll('[role="tab"]')]
  expect(tabs.map((tab) => tab.getAttribute("aria-label") ?? tab.textContent)).toEqual([
    "Claude",
    "Codex",
    "nova",
  ])
})

it("says each model's context in short", () => {
  render()
  open()

  expect(document.body.textContent).toContain("Opus 51M context")
  expect(document.body.textContent).not.toContain("1.1M")
})

it("hands back the agent and model that were picked", () => {
  const onChoose = render()
  open()

  const option = [...document.body.querySelectorAll<HTMLElement>('[role="option"]')].find(
    (item) => item.textContent?.startsWith("Opus 5"),
  )
  if (!option) throw new Error("no Opus option")
  act(() => option.click())

  expect(onChoose).toHaveBeenCalledExactlyOnceWith({
    agent: "claude",
    model: "claude-opus-5",
  })
})
