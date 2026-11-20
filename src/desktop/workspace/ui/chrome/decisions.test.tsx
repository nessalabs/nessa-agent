// @vitest-environment jsdom
/**
 * Decisions in Chat context (`decisions.tsx`), through the whole shell: the
 * latest three, waiting first, "See all" and a row opening the dialog, its
 * filters, a waiting decision answered there with the transcript's card,
 * and the list read again when the chat's ask changes.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { ClockProvider } from "../../../../clock"
import {
  createWidgetRegistry,
  WidgetRegistryProvider,
  type WidgetPlugin,
} from "../../../widgets"
import type { DecisionSource } from "../../application/ports"
import {
  followWorkspace,
  loadWorkspace,
  openSession,
} from "../../adapters/store/commands"
import { selectSession } from "../../adapters/store/selectors"
import type { Decision, DecisionOutcome } from "../../model/decisions"
import type { OwnerAsk } from "../../model/workspace-index"
import {
  controlledAnimationFrames,
  fakeSource,
  flushAnimationFrames,
  settle,
  testStore,
  type AnimationFrames,
  type FakeSource,
} from "../../testing"
import { ThreeColumns } from "../layouts/three-columns"
import { DecisionsProvider } from "./decisions-source"

const minute = 60_000
const now = 100 * minute

let host: HTMLDivElement
let root: Root
let animation: AnimationFrames

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  animation = controlledAnimationFrames()
  class Observer {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer })
  Element.prototype.scrollTo ??= () => {}
  Element.prototype.scrollIntoView ??= () => {}
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  animation.restore()
})

function ask(askId: string): OwnerAsk {
  return {
    askId,
    question: `Question ${askId}?`,
    choices: [
      { label: "Ship", consequence: "Goes out today." },
      { label: "Wait", consequence: "Goes out Monday." },
    ],
    recommended: 0,
    reason: "It is ready.",
  }
}

function decision(askId: string, askedAt: number, outcome: DecisionOutcome): Decision {
  return { ask: ask(askId), askedAtMs: askedAt * minute, outcome }
}

const settled: readonly Decision[] = [
  decision("d1", 60, { state: "decided", text: "Ship", choice: 0, atMs: 61 * minute }),
  decision("d2", 70, { state: "replaced", atMs: 75 * minute }),
  decision("d3", 75, {
    state: "decided",
    text: "Not yet. Ask me Monday.",
    atMs: 80 * minute,
  }),
  decision("d4", 50, { state: "decided", text: "Wait", choice: 1, atMs: 52 * minute }),
]

/** A source that answers with `list()` as it is when asked, and counts the reads. */
function decisionsOf(list: () => readonly Decision[] | Error) {
  const reads: string[] = []
  const source: DecisionSource = {
    decisions: async (sessionId) => {
      reads.push(sessionId)
      const answer = list()
      if (answer instanceof Error) throw answer
      return answer
    },
  }
  return { source, reads }
}

async function render(source: DecisionSource, fake: FakeSource = fakeSource()) {
  const store = testStore(fake)
  await store.dispatch(loadWorkspace())
  await settle()
  store.dispatch(followWorkspace())
  store.dispatch(openSession({ sessionId: "a" }))
  await settle()
  await act(async () =>
    root.render(
      <Provider store={store}>
        <WidgetRegistryProvider registry={createWidgetRegistry<WidgetPlugin>([])}>
          <ClockProvider now={() => now} elapsed={() => now}>
            <DecisionsProvider source={source}>
              <ThreeColumns hostKind="browser" browserSurface />
            </DecisionsProvider>
          </ClockProvider>
        </WidgetRegistryProvider>
      </Provider>,
    ),
  )
  await frames()
  await click(host.querySelector("[data-chat-context-button]"))
  await settle()
  await frames()
  return store
}

const frames = () => flushAnimationFrames(animation, act)
const click = async (element: Element | null | undefined) => {
  if (!element) throw new Error("nothing to click")
  await act(async () => (element as HTMLElement).click())
  await frames()
}
const section = () => host.querySelector('[data-chat-context-section="decisions"]')
const rows = () =>
  [...(section()?.querySelectorAll("[data-decision]") ?? [])].map((row) =>
    row.getAttribute("data-decision"),
  )
const dialog = () => document.querySelector("[data-decisions-dialog]")
const listed = () =>
  [...(dialog()?.querySelectorAll("li[data-decision]") ?? [])].map((row) =>
    row.getAttribute("data-decision"),
  )
const pressedFilter = () =>
  dialog()
    ?.querySelector(
      '[data-filter][aria-pressed="true"], [data-filter][aria-checked="true"], [data-filter][data-state="on"]',
    )
    ?.getAttribute("data-filter")

describe("Decisions in Chat context", () => {
  it("lists the latest three, newest first, and See all names how many more", async () => {
    const { source } = decisionsOf(() => settled)
    await render(source)
    expect(section()?.textContent).toContain("Decisions")
    expect(rows()).toEqual(["d3", "d2", "d1"])
    expect(section()?.textContent).toContain("Your own words · 20m ago")
    expect(section()?.querySelector("[data-decisions-all]")?.textContent).toBe(
      "1 more · See all",
    )
  })

  it("puts what waits on the person first, and opens the dialog on it under To decide", async () => {
    const fake = fakeSource()
    const { source } = decisionsOf(() => [
      ...settled,
      decision("w", 40, { state: "waiting" }),
    ])
    const store = await render(source, fake)
    expect(rows()).toEqual(["w", "d3", "d2"])
    await click(section()?.querySelector('[data-decision="w"]'))
    expect(dialog()).not.toBeNull()
    expect(pressedFilter()).toBe("waiting")
    expect(listed()).toEqual(["w"])
    // Not the chat's ask now: drawn as a record, never a card that sends.
    expect(dialog()?.querySelector("[data-owner-ask]")).toBeNull()
    expect(selectSession(store.getState(), "a")?.ask).toBeUndefined()
  })

  it("See all opens on All; Decided keeps the replaced ones too", async () => {
    const { source } = decisionsOf(() => settled)
    await render(source)
    await click(section()?.querySelector("[data-decisions-all]"))
    expect(pressedFilter()).toBe("all")
    expect(listed()).toEqual(["d3", "d2", "d1", "d4"])
    await click(dialog()?.querySelector('[data-filter="waiting"]'))
    expect(listed()).toEqual([])
    expect(dialog()?.textContent).toContain("Nothing waits on you")
    await click(dialog()?.querySelector('[data-filter="decided"]'))
    expect(listed()).toEqual(["d3", "d2", "d1", "d4"])
  })

  it("shows a settled decision's choices, the one picked, and what was said", async () => {
    const { source } = decisionsOf(() => settled)
    await render(source)
    await click(section()?.querySelector('[data-decision="d3"]'))
    const detail = dialog()?.querySelector('[data-decision-detail="d3"]')
    expect(detail?.textContent).toContain("Decided: Your own words")
    expect(detail?.querySelector("blockquote")?.textContent).toBe(
      "Not yet. Ask me Monday.",
    )
    await click(dialog()?.querySelector('li[data-decision="d1"] > button'))
    const picked = dialog()?.querySelector('[data-decision-detail="d1"] [data-chosen]')
    expect(picked?.textContent).toContain("Ship")
  })

  it("answers the chat's waiting ask from the dialog with the transcript's own card", async () => {
    const fake = fakeSource()
    const { source } = decisionsOf(() => [decision("live", 90, { state: "waiting" })])
    const store = await render(source, fake)
    const summary = selectSession(store.getState(), "a")
    if (!summary) throw new Error("no session a")
    await act(async () =>
      fake.emit({
        kind: "session",
        session: { ...summary, ask: ask("live"), revision: summary.revision + 1 },
      }),
    )
    await settle()
    await frames()
    await click(section()?.querySelector('[data-decision="live"]'))
    expect(dialog()?.querySelector('[data-owner-ask="live"]')).not.toBeNull()
  })

  it("reads again when the chat's ask changes, and says so when a read fails", async () => {
    const fake = fakeSource()
    let answer: readonly Decision[] | Error = settled
    const { source, reads } = decisionsOf(() => answer)
    const store = await render(source, fake)
    expect(reads).toEqual(["a"])
    answer = new Error("offline")
    const summary = selectSession(store.getState(), "a")
    if (!summary) throw new Error("no session a")
    await act(async () =>
      fake.emit({
        kind: "session",
        session: { ...summary, ask: ask("next"), revision: summary.revision + 1 },
      }),
    )
    await settle()
    await frames()
    expect(reads).toEqual(["a", "a"])
    // The rows read before stay, the dialog with them, and Try again reads again.
    expect(rows()).toEqual(["d3", "d2", "d1"])
    expect(section()?.textContent).toContain("Could not refresh")
    answer = settled.slice(0, 2)
    await click(section()?.querySelector("[data-decisions-failed] button"))
    await settle()
    await frames()
    expect(reads).toEqual(["a", "a", "a"])
    expect(rows()).toEqual(["d2", "d1"])
    expect(section()?.querySelector("[data-decisions-failed]")).toBeNull()
  })

  it("says when the first read fails, and reads again on Try again", async () => {
    let answer: readonly Decision[] | Error = new Error("offline")
    const { source, reads } = decisionsOf(() => answer)
    await render(source)
    expect(section()?.textContent).toContain("Decisions could not be read")
    answer = settled
    await click(section()?.querySelector("[data-decisions-failed] button"))
    await settle()
    await frames()
    expect(reads).toEqual(["a", "a"])
    expect(rows()).toEqual(["d3", "d2", "d1"])
  })

  it("is not there beside no source of decisions", async () => {
    const store = testStore(fakeSource())
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(openSession({ sessionId: "a" }))
    await settle()
    await act(async () =>
      root.render(
        <Provider store={store}>
          <WidgetRegistryProvider registry={createWidgetRegistry<WidgetPlugin>([])}>
            <ClockProvider now={() => now} elapsed={() => now}>
              <ThreeColumns hostKind="browser" browserSurface />
            </ClockProvider>
          </WidgetRegistryProvider>
        </Provider>,
      ),
    )
    await frames()
    await click(host.querySelector("[data-chat-context-button]"))
    expect(host.querySelector("[data-chat-context]")).not.toBeNull()
    expect(section()).toBeNull()
  })
})
