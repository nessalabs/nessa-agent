// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { PanelOwnerAsk } from "./owner-ask"

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

const ask = {
  askId: "0123456789abcdef0123456789abcdef",
  question: "Ship it today?",
  choices: [
    { label: "Ship", consequence: "It goes out today." },
    { label: "Wait", consequence: "It waits for Monday." },
  ],
  recommended: 0,
  reason: "Every check passed.",
  askedAtMs: 1,
}

const choice = (label: string): HTMLElement => {
  const found = container.querySelector<HTMLElement>(`[data-owner-ask-choice="${label}"]`)
  if (!found) throw new Error(`no choice ${label}`)
  return found
}

it("offers every choice again when the gateway did not take the answer", async () => {
  const sent: string[] = []
  let settle!: (taken: boolean) => void
  act(() => {
    root.render(
      <PanelOwnerAsk
        ask={ask}
        submit={(text) => {
          sent.push(text)
          return new Promise((resolve) => (settle = resolve))
        }}
      />,
    )
  })
  act(() => choice("Wait").click())
  expect(sent).toEqual(["Wait"])
  expect(container.querySelector("[aria-busy='true']")).not.toBeNull()
  await act(async () => settle(false))
  expect(container.querySelector("[data-owner-ask-failure]")?.textContent).toContain(
    "was not sent",
  )
  expect(container.querySelector("[aria-busy='true']")).toBeNull()
  act(() => choice("Ship").click())
  expect(sent).toEqual(["Wait", "Ship"])
})
