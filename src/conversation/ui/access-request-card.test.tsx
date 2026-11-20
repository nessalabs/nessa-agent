// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { RaiseOutcome } from "../model/access-approval"
import { forgetChatApprovals } from "./access-approvals"
import { AccessRequestHost } from "./access-request-card"

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
  forgetChatApprovals()
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
    underway,
    working = false,
    revision = 1,
  }: {
    ask: Shown
    runsAt: Mode | undefined
    underway?: Mode
    working?: boolean
    revision?: number
  }) =>
    act(() =>
      root.render(
        <AccessRequestHost
          chat="c"
          ask={ask}
          runsAt={runsAt}
          underway={underway}
          working={working}
          revision={revision}
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

/** Approves `access` at revision 1, Ask, and lets the answer go at Full. */
async function approved() {
  const round = host()
  round.draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  round.draw({ ask: access, runsAt: "full", revision: 2 })
  await round.answer(true)
  return round
}

it("keeps the card through the answer, even as the list lets the ask go", async () => {
  const { sent, draw, answer } = host()
  draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  draw({ ask: access, runsAt: "full", revision: 2 })
  expect(sent).toEqual(["Approve"])
  const drawn = card()
  // The answer's own write takes the ask off the row before the send returns.
  draw({ ask: undefined, runsAt: "full", revision: 3 })
  expect(card()).toBe(drawn)
  await answer(true)
  expect(card()).toBe(drawn)
  expect(card()?.dataset.state).toBe("approved")
})

it("sends nothing when the ask leaves in the same look the access arrives", async () => {
  const { sent, draw } = host()
  draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  draw({ ask: undefined, runsAt: "full", revision: 2 })
  expect(sent).toEqual([])
  expect(card()).toBeNull()
  // Nor as the answer to an ask that replaced it.
  const again = host()
  again.draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  again.draw({ ask: question, runsAt: "full", revision: 2 })
  expect(again.sent).toEqual([])
})

it("lets the approved card go for good when another ask comes", async () => {
  const { draw } = await approved()
  draw({ ask: undefined, runsAt: "full", revision: 3 })
  expect(card()?.dataset.state).toBe("approved")
  draw({ ask: question, runsAt: "full", revision: 4 })
  expect(card()).toBeNull()
  draw({ ask: undefined, runsAt: "full", revision: 5 })
  expect(card()).toBeNull()
})

it("lets the approved card go for good when the access is lowered, raised or not again", async () => {
  const { draw } = await approved()
  draw({ ask: undefined, runsAt: "ask", revision: 3 })
  expect(card()).toBeNull()
  draw({ ask: undefined, runsAt: "full", revision: 4 })
  expect(card()).toBeNull()
})

it("waits through an old view, then ends when a newer one settles elsewhere", async () => {
  const { sent, draw } = host()
  draw({ ask: access, runsAt: "ask", revision: 7 })
  await act(async () => button("Approve").click())
  // The view the raise was taken at still says Ask: not a reason to end.
  draw({ ask: access, runsAt: "ask", revision: 7 })
  expect(card()?.dataset.state).toBe("approving")
  expect(container.textContent).toContain("Waiting for the access to change.")
  // A newer view settles at Auto, with no change said to be on its way.
  draw({ ask: access, runsAt: "auto", revision: 8 })
  expect(card()?.dataset.state).toBe("asking")
  expect(container.textContent).toContain("Access didn’t change to this.")
  expect(sent).toEqual([])
})

it("waits for the turn's end, and ends when the change on its way is to another access", async () => {
  const { sent, draw } = host("pending")
  draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  draw({ ask: access, runsAt: "ask", underway: "full", working: true, revision: 2 })
  expect(card()?.dataset.state).toBe("approving")
  expect(container.textContent).toContain("Access changes when the agent stops.")
  expect(container.textContent).not.toContain("You can approve")
  draw({ ask: access, runsAt: "ask", underway: "auto", working: true, revision: 3 })
  expect(card()?.dataset.state).toBe("asking")
  expect(container.textContent).toContain("Access didn’t change to this.")
  expect(sent).toEqual([])
})

it("says another change is on its way, rather than a refusal", async () => {
  const { draw } = host("changing")
  draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  expect(container.textContent).toContain("Another change to this chat is on its way.")
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

it("sends when the view shows the access before the raise returns, as the panel's does", async () => {
  const sent: string[] = []
  const draw = (runsAt: Mode, revision: number, raise: () => Promise<RaiseOutcome>) =>
    act(() =>
      root.render(
        <AccessRequestHost
          chat="c"
          ask={access}
          runsAt={runsAt}
          underway={undefined}
          working={false}
          revision={revision}
          raise={raise}
          answer={(text) => {
            sent.push(text)
            return Promise.resolve(true)
          }}
        />,
      ),
    )
  // The raise reads the conversation again before it returns: the view
  // already runs at Full, and no later look comes.
  const raise = async () => {
    await draw("full", 2, raise)
    return "taken" as const
  }
  draw("ask", 1, raise)
  await act(async () => button("Approve").click())
  expect(sent).toEqual(["Approve"])
  expect(card()?.dataset.state).toBe("approved")
})

it("keeps a waiting Approve when the card leaves the screen and comes back", async () => {
  const { sent, draw } = host("pending")
  draw({ ask: access, runsAt: "ask" })
  await act(async () => button("Approve").click())
  draw({ ask: access, runsAt: "ask", underway: "full", working: true, revision: 2 })
  // Another tab: the card is not drawn.
  act(() => root.render(<></>))
  // Back, once the turn ended and the chat runs at Full.
  draw({ ask: access, runsAt: "full", revision: 3 })
  expect(sent).toEqual(["Approve"])
})
