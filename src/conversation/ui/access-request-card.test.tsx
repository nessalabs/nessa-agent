// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { AccessRequestHost, type RaiseOutcome } from "./access-request-card"

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

const access = {
  askId: "fedcba9876543210fedcba9876543210",
  question: "Allow full access in this chat?",
  choices: [
    { label: "Approve", consequence: "Raises this chat to Full access." },
    { label: "Not now", consequence: "Keeps this chat's access as it is." },
  ],
  recommended: 1,
  reason: "The toolchain installs outside the project.",
  access: {
    mode: "full" as const,
    grants: [
      "Agents in this chat can run any command",
      "They won't ask you before they act",
      "You can undo this from Access, under the message box",
    ],
  },
}
const question = {
  askId: "0123456789abcdef0123456789abcdef",
  question: "Ship it today?",
  choices: [
    { label: "Ship", consequence: "It goes out today." },
    { label: "Wait", consequence: "It waits for Monday." },
  ],
  recommended: 0,
  reason: "Every check passed.",
}

type Mode = "ask" | "auto" | "full"
type Shown = typeof access | typeof question | undefined

/** A host as either window is: the props it is drawn with, and what it sent. */
function host(raised: RaiseOutcome = "taken") {
  const sent: string[] = []
  let answered: (sent: boolean) => void = () => {}
  const draw = ({
    ask,
    runsAt,
    heading,
    working = false,
  }: {
    ask: Shown
    runsAt: Mode | undefined
    heading?: Mode
    working?: boolean
  }) =>
    act(() =>
      root.render(
        <AccessRequestHost
          ask={ask}
          runsAt={runsAt}
          heading={heading}
          working={working}
          raise={() => Promise.resolve(raised)}
          answer={(text) => {
            sent.push(text)
            return new Promise((resolve) => (answered = resolve))
          }}
        />,
      ),
    )
  return { sent, draw, answer: (ok: boolean) => act(async () => answered(ok)) }
}

const card = () => container.querySelector<HTMLElement>("[data-access-request]")
const button = (name: string): HTMLButtonElement => {
  const found = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
    (each) => each.textContent === name,
  )
  if (!found) throw new Error(`no button ${name}`)
  return found
}

it("keeps the card through the answer, even as the list lets the ask go", async () => {
  const { sent, draw, answer } = host()
  draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  draw({ ask: access, runsAt: "full" })
  expect(sent).toEqual(["Approve"])
  const drawn = card()
  // The answer's own write takes the ask off the row before the send returns.
  draw({ ask: undefined, runsAt: "full" })
  expect(card()).toBe(drawn)
  expect(card()?.dataset.state).toBe("approved")
  await answer(true)
  expect(card()).toBe(drawn)
})

it("lets the approved card go when another ask comes, and when access is lowered", async () => {
  const { draw, answer } = host()
  draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  draw({ ask: access, runsAt: "full" })
  await answer(true)
  draw({ ask: undefined, runsAt: "full" })
  expect(card()?.dataset.state).toBe("approved")
  // A later ordinary question: its own card's, and the red one is gone for good.
  draw({ ask: question, runsAt: "full" })
  expect(card()).toBeNull()
  draw({ ask: undefined, runsAt: "full" })
  expect(card()).toBeNull()

  const again = host()
  again.draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  again.draw({ ask: access, runsAt: "full" })
  await again.answer(true)
  again.draw({ ask: undefined, runsAt: "ask" })
  expect(card()).toBeNull()
})

it("ends the wait when the access is headed somewhere else, and offers Approve again", async () => {
  const { sent, draw } = host("pending")
  draw({ ask: access, runsAt: "ask", heading: undefined })
  await act(async () => button("Approve").click())
  draw({ ask: access, runsAt: "ask", heading: "full", working: true })
  expect(card()?.dataset.state).toBe("approving")
  // Waiting for the turn to end is not a reason Approve can't be pressed.
  expect(container.textContent).not.toContain("once the agent stops")
  // The person picked Auto in the composer since.
  draw({ ask: access, runsAt: "ask", heading: "auto", working: true })
  expect(card()?.dataset.state).toBe("asking")
  expect(container.textContent).toContain("Access didn’t change to this.")
  expect(sent).toEqual([])
})

it("sends Not now as the answer, and changes nothing", async () => {
  const { sent, draw, answer } = host()
  draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Not now").click())
  expect(sent).toEqual(["Not now"])
  await answer(false)
  expect(container.textContent).toContain("Not now was not sent.")
  expect(card()?.dataset.state).toBe("asking")
})
