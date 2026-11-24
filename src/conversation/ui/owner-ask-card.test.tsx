// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { OwnerAskShown } from "../model/owner-ask"
import { OwnerAskCard } from "./owner-ask-card"

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

const ship = { label: "Ship", consequence: "It goes out today." }
const wait = { label: "Wait", consequence: "It waits for Monday." }
const plain: OwnerAskShown = {
  askId: "0123456789abcdef0123456789abcdef",
  question: "Ship it today?",
  choices: [ship, wait],
  recommended: 0,
  reason: "Every check passed.",
}

const draw = (ask: OwnerAskShown) =>
  act(() =>
    root.render(
      <OwnerAskCard ask={ask} sending={null} failure={null} onChoose={() => {}} />,
    ),
  )

const undos = () =>
  [...container.querySelectorAll<HTMLElement>("[data-owner-ask-undo]")].map(
    (badge) => badge.textContent,
  )

it("shows the summary under the question, and nothing for an ask without one", () => {
  draw(plain)
  expect(container.querySelector("[data-owner-ask-summary]")).toBeNull()
  draw({ ...plain, summary: "The Safari export fix." })
  expect(container.querySelector("[data-owner-ask-summary]")?.textContent).toBe(
    "The Safari export fix.",
  )
})

it("says how hard each choice is to undo only when that differs between them", () => {
  draw(plain)
  expect(undos()).toEqual([])
  draw({
    ...plain,
    choices: [
      { ...ship, undo: "easy" },
      { ...wait, undo: "easy" },
    ],
  })
  expect(undos()).toEqual([])
  draw({ ...plain, choices: [{ ...ship, undo: "no" }, wait] })
  expect(undos()).toEqual([])
  draw({
    ...plain,
    choices: [
      { ...ship, undo: "no" },
      { ...wait, undo: "easy" },
    ],
  })
  expect(undos()).toEqual(["Can't undo", "Easy to undo"])
})
