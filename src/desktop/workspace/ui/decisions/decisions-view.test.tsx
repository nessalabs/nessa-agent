// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { ClockProvider } from "../../../../clock"
import { followWorkspace, loadWorkspace } from "../../adapters/store/commands"
import type { OwnerAsk } from "../../model/workspace-index"
import { fakeSource, settle, summary, testIndex, testStore } from "../../testing"
import type { DecisionSource } from "../../application/ports"
import { DecisionsProvider } from "../chrome/decisions-source"
import { DecisionsHost } from "./decisions-view"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const ask = (askId: string, undo: "easy" | "no"): OwnerAsk => ({
  askId,
  question: `Question ${askId}?`,
  summary: `What ${askId} is about.`,
  choices: [
    { label: `Yes ${askId}`, consequence: "It goes ahead.", undo },
    { label: `No ${askId}`, consequence: "It stops.", undo: "easy" },
  ],
  recommended: 0,
  reason: `Why yes on ${askId}.`,
})

async function open(on?: string, decisions?: DecisionSource) {
  const index = testIndex()
  const source = fakeSource({
    ...index,
    sessions: index.sessions.map((session) =>
      session.id === "a"
        ? summary("a", "desktop", 100, "needs-you", { ask: ask("one", "easy") })
        : session.id === "b"
          ? summary("b", "desktop", 200, "needs-you", { ask: ask("two", "no") })
          : session,
    ),
  })
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  store.dispatch(followWorkspace())
  await settle()
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1_000} elapsed={() => 1_000}>
          <DecisionsProvider source={decisions}>
            <DecisionsHost open on={on} onClose={() => {}} />
          </DecisionsProvider>
        </ClockProvider>
      </Provider>,
    ),
  )
  return source
}

/** The one element `selector` names under `from`, or a failure saying which. */
function find<T extends Element = HTMLElement>(
  selector: string,
  from: ParentNode = document,
): T {
  const found = from.querySelector<T>(selector)
  if (!found) throw new Error(`nothing matches ${selector}`)
  return found
}

const view = () => find("[data-decisions-view]")
const sends = (source: ReturnType<typeof fakeSource>) =>
  source.calls.filter((call) => call[0] === "send")

it("opens on the named chat's decision, with its brief beside it", async () => {
  await open("b")
  expect(
    view()
      .querySelector("[data-decisions-decide]")
      ?.getAttribute("data-decisions-decide"),
  ).toBe("two")
  const brief = find("[data-decisions-brief]", view())
  expect(brief.textContent).toContain("What two is about.")
  expect(brief.textContent).toContain("Why yes on two.")
  // Undo differs between its choices, so the options compare on it.
  expect(brief.querySelector('[data-fact="undo"]')).not.toBeNull()
})

it("sends nothing on a pick, and the picked label on Decide, then shows the next", async () => {
  const source = await open("a")
  const send = find<HTMLButtonElement>("[data-decisions-send]", view())
  expect(send.disabled).toBe(true)
  await act(async () =>
    find<HTMLElement>('[data-decisions-choice="No one"]', view()).click(),
  )
  expect(sends(source)).toEqual([])
  expect(send.disabled).toBe(false)
  await act(async () => send.click())
  await act(async () => settle(10))
  expect(sends(source)).toHaveLength(1)
  expect(sends(source)[0]?.[1]).toMatchObject({ sessionId: "a", text: "No one" })
  expect(
    view()
      .querySelector("[data-decisions-decide]")
      ?.getAttribute("data-decisions-decide"),
  ).toBe("two")
  expect(view().querySelector("[data-decisions-progress]")?.textContent).toContain(
    "1 of 2 decided",
  )
})

it("sends the person's own words for Something else", async () => {
  const source = await open("a")
  await act(async () => find<HTMLElement>("[data-decisions-choice-own]", view()).click())
  const input = find<HTMLInputElement>("[data-decisions-own]", view())
  expect(document.activeElement).toBe(input)
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(
      input,
      "Ask Dana first",
    )
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
  await act(async () => find<HTMLButtonElement>("[data-decisions-send]", view()).click())
  await act(async () => settle(10))
  expect(sends(source)[0]?.[1]).toMatchObject({ sessionId: "a", text: "Ask Dana first" })
})

it("ticks only easy-to-undo suggestions in Accept all, and sends each ticked one", async () => {
  const source = await open()
  await act(async () =>
    find<HTMLButtonElement>("[data-decisions-accept-all]", view()).click(),
  )
  const boxes = [
    ...view().querySelectorAll<HTMLInputElement>(
      '[data-decisions-accept] input[type="checkbox"]',
    ),
  ]
  expect(boxes.map((box) => box.checked)).toEqual([true, false])
  await act(async () =>
    find<HTMLButtonElement>("[data-decisions-accept-send]", view()).click(),
  )
  await act(async () => settle(10))
  expect(sends(source).map((call) => call[1])).toEqual([
    expect.objectContaining({ sessionId: "a", text: "Yes one" }),
  ])
})

it("leaves a replaced ask unticked in Accept all, so nothing unseen is sent", async () => {
  const source = await open()
  await act(async () =>
    find<HTMLButtonElement>("[data-decisions-accept-all]", view()).click(),
  )
  // Chat a's agent asks something new while the list is open: its
  // suggestion can't be undone, and the person never ticked it.
  act(() =>
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 300, "needs-you", {
        ask: ask("three", "no"),
        revision: 2,
      }),
    }),
  )
  await act(async () => settle(10))
  const boxes = [
    ...view().querySelectorAll<HTMLInputElement>(
      '[data-decisions-accept] input[type="checkbox"]',
    ),
  ]
  expect(boxes.map((box) => box.checked)).toEqual([false, false])
  expect(find<HTMLButtonElement>("[data-decisions-accept-send]", view()).disabled).toBe(
    true,
  )
})

it("moves focus to the next decision's choices after Decide", async () => {
  await open("a")
  await act(async () => find('[data-decisions-choice="No one"]', view()).click())
  await act(async () => find<HTMLButtonElement>("[data-decisions-send]", view()).click())
  await act(async () => settle(10))
  const next = find('[data-decisions-decide="two"]', view())
  expect(next.contains(document.activeElement)).toBe(true)
  expect(document.activeElement?.tagName).toBe("INPUT")
})

it("shows auto-pick's countdown and Wait for me beside the choices", async () => {
  const pauses: [string, string, boolean][] = []
  const decisions: DecisionSource = {
    decisions: async () => [
      {
        ask: ask("one", "easy"),
        askedAtMs: 0,
        outcome: { state: "waiting" },
        autoPick: { state: "due", atMs: 1_000 + 65_000 },
      },
    ],
    pauseAutoPick: async (sessionId, askId, paused) => {
      pauses.push([sessionId, askId, paused])
      return true
    },
  }
  await open("a", decisions)
  await act(async () => settle(10))
  const line = find("[data-decisions-decide] [data-auto-pick-line]", view())
  expect(line.getAttribute("data-auto-pick-line")).toBe("due")
  await act(async () =>
    find<HTMLButtonElement>('[data-auto-pick-hold="pause"]', line).click(),
  )
  await act(async () => settle(10))
  expect(pauses).toEqual([["a", "one", true]])
})
