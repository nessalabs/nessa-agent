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

/** Newest first, as the gateway lists them. */
const settled: readonly Decision[] = [
  decision("d3", 75, {
    state: "decided",
    text: "Not yet. Ask me Monday.",
    atMs: 80 * minute,
  }),
  decision("d2", 70, { state: "replaced", atMs: 75 * minute }),
  decision("d1", 60, { state: "decided", text: "Ship", choice: 0, atMs: 61 * minute }),
  decision("d4", 50, { state: "decided", text: "Wait", choice: 1, atMs: 52 * minute }),
]

/** A source that answers with `list()` as it is when asked, and counts the reads and pauses. */
function decisionsOf(list: () => readonly Decision[] | Error) {
  const reads: string[] = []
  const pauses: [string, string, boolean][] = []
  const source: DecisionSource = {
    decisions: async (sessionId) => {
      reads.push(sessionId)
      const answer = list()
      if (answer instanceof Error) throw answer
      return answer
    },
    pauseAutoPick: async (sessionId, askId, paused) => {
      pauses.push([sessionId, askId, paused])
      return true
    },
  }
  return { source, reads, pauses }
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
    expect(detail?.textContent).toContain("You answered in your own words")
    expect(detail?.querySelector("blockquote")?.textContent).toBe(
      "Not yet. Ask me Monday.",
    )
    await click(dialog()?.querySelector('li[data-decision="d1"] > button'))
    const picked = dialog()?.querySelector('[data-decision-detail="d1"] [data-chosen]')
    expect(picked?.textContent).toContain("Ship")
  })

  it("answers the chat's waiting ask from the expanded card, with a note", async () => {
    const fake = fakeSource()
    const detailed: Decision = {
      ask: {
        ...ask("live"),
        summary: "The fix is reviewed.",
        choices: [
          {
            label: "Ship",
            consequence: "Goes out today.",
            undo: "easy",
            effort: "small",
            pros: ["Fixed today"],
          },
          {
            label: "Wait",
            consequence: "Goes out Monday.",
            undo: "costly",
            cons: ["Users wait"],
          },
        ],
      },
      askedAtMs: 90 * minute,
      outcome: { state: "waiting" },
    }
    const { source } = decisionsOf(() => [detailed])
    const store = await render(source, fake)
    const summary = selectSession(store.getState(), "a")
    if (!summary) throw new Error("no session a")
    await act(async () =>
      fake.emit({
        kind: "session",
        session: { ...summary, ask: detailed.ask, revision: summary.revision + 1 },
      }),
    )
    await settle()
    await frames()
    await click(section()?.querySelector('[data-decision="live"]'))
    const detail = dialog()?.querySelector('[data-decision-detail="live"]')
    expect(detail?.querySelector("h3")?.textContent).toBe("Question live?")
    expect(detail?.textContent).toContain("The fix is reviewed.")
    expect(detail?.textContent).toContain("Fixed today")
    expect(detail?.textContent).toContain("Users wait")
    expect(detail?.textContent).toContain("Small effort")
    expect(detail?.querySelector('[data-undo="costly"]')?.textContent).toBe(
      "Costly to undo",
    )
    expect(detail?.textContent).toContain("Why Ship")
    const send = () => document.querySelector<HTMLButtonElement>("[data-decisions-send]")
    expect(send()?.textContent).toBe("Pick a choice")
    expect(send()?.disabled).toBe(true)
    await click(detail?.querySelector('[data-decisions-pick="1"]'))
    expect(send()?.textContent).toBe("Answer: Wait")
    const note = document.querySelector<HTMLInputElement>("[data-decisions-note]")
    if (!note) throw new Error("no note")
    await act(async () => {
      const set = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )?.set
      set?.call(note, "Monday is fine")
      note.dispatchEvent(new Event("input", { bubbles: true }))
    })
    await click(send())
    await settle()
    const sent = fake.calls.filter(([method]) => method === "send")
    expect(sent).toHaveLength(1)
    expect(sent[0]?.[1]).toMatchObject({ sessionId: "a", text: "Wait. Monday is fine" })
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
    answer = settled.slice(1, 3)
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

  it("counts down to auto-pick on the chat's ask, and pauses it in one tap", async () => {
    const fake = fakeSource()
    let held = false
    const waiting = (): Decision => ({
      ...decision("live", 99, { state: "waiting" }),
      autoPick: held
        ? { state: "paused" }
        : { state: "due", atMs: now + 4 * minute + 5_000 },
    })
    const { source, pauses } = decisionsOf(() => [waiting()])
    const store = await render(source, fake)
    expect(section()?.textContent).toContain("Waiting on you · auto-picks in 4:05")
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
    const line = () => host.querySelector('[data-owner-ask="live"] [data-auto-pick-line]')
    expect(line()?.textContent).toContain("Picks Ship in 4:05")
    held = true
    await click(line()?.querySelector('[data-auto-pick-hold="pause"]'))
    await settle()
    await frames()
    expect(pauses).toEqual([["a", "live", true]])
    expect(line()?.getAttribute("data-auto-pick-line")).toBe("paused")
    expect(line()?.textContent).toContain("Auto-pick paused")
    held = false
    await click(line()?.querySelector('[data-auto-pick-hold="resume"]'))
    await settle()
    await frames()
    expect(pauses).toEqual([
      ["a", "live", true],
      ["a", "live", false],
    ])
    expect(line()?.getAttribute("data-auto-pick-line")).toBe("due")
  })

  it("says a decision was auto-picked, and Change my answer sends another choice", async () => {
    const fake = fakeSource()
    const { source } = decisionsOf(() => [
      decision("auto", 90, {
        state: "decided",
        text: "Ship. Picked for you on “Question auto?”: no answer in 5 minutes.",
        choice: 0,
        auto: true,
        atMs: 95 * minute,
      }),
    ])
    await render(source, fake)
    expect(section()?.textContent).toContain("Auto-picked Ship · 5m ago")
    await click(section()?.querySelector('[data-decision="auto"]'))
    const detail = () => dialog()?.querySelector('[data-decision-detail="auto"]')
    expect(detail()?.textContent).toContain("Auto-picked Ship")
    expect(detail()?.textContent).toContain("No answer in time")
    // The pick's own words are not shown as something the person said.
    expect(detail()?.querySelector("blockquote")).toBeNull()
    await click(detail()?.querySelector("[data-change-answer]"))
    const offered = [
      ...(detail()?.querySelectorAll("[data-change-answer-choice]") ?? []),
    ].map((choice) => choice.getAttribute("data-change-answer-choice"))
    expect(offered).toEqual(["Wait"])
    await click(detail()?.querySelector('[data-change-answer-choice="Wait"]'))
    await settle()
    await frames()
    const sent = fake.calls.filter(([method]) => method === "send")
    expect(sent).toHaveLength(1)
    expect(sent[0]?.[1]).toMatchObject({
      sessionId: "a",
      text: "Wait. Changed from the auto-pick on “Question auto?”",
    })
    expect(detail()?.querySelector("[data-change-answer-sent]")?.textContent).toBe(
      "Sent: Wait",
    )
  })

  it("offers no Change my answer while a newer ask waits, since it would answer that one", async () => {
    const { source } = decisionsOf(() => [
      decision("next", 97, { state: "waiting" }),
      decision("auto", 90, {
        state: "decided",
        text: "Ship. Picked for you on “Question auto?”: no answer in 5 minutes.",
        choice: 0,
        auto: true,
        atMs: 95 * minute,
      }),
    ])
    await render(source)
    await click(section()?.querySelector('[data-decision="auto"]'))
    const detail = dialog()?.querySelector('[data-decision-detail="auto"]')
    expect(detail?.textContent).toContain("Auto-picked Ship")
    expect(detail?.querySelector("[data-change-answer]")).toBeNull()
  })

  it("offers no Change my answer once the chat's row shows a newer ask, before the read catches up", async () => {
    const fake = fakeSource()
    const { source } = decisionsOf(() => [
      decision("auto", 90, {
        state: "decided",
        text: "Ship. Picked for you on “Question auto?”: no answer in 5 minutes.",
        choice: 0,
        auto: true,
        atMs: 95 * minute,
      }),
    ])
    const store = await render(source, fake)
    await click(section()?.querySelector('[data-decision="auto"]'))
    const detail = () => dialog()?.querySelector('[data-decision-detail="auto"]')
    expect(detail()?.querySelector("[data-change-answer]")).not.toBeNull()
    const summary = selectSession(store.getState(), "a")
    if (!summary) throw new Error("no session a")
    await act(async () =>
      fake.emit({
        kind: "session",
        session: { ...summary, ask: ask("next"), revision: summary.revision + 1 },
      }),
    )
    await frames()
    expect(detail()?.querySelector("[data-change-answer]")).toBeNull()
  })

  it("reads again when a due pick has not landed a few seconds past its time", async () => {
    const fake = fakeSource()
    const { source, reads } = decisionsOf(() => [
      {
        ...decision("live", 90, { state: "waiting" }),
        autoPick: { state: "due", atMs: now - 10_000 },
      },
    ])
    const store = await render(source, fake)
    const summary = selectSession(store.getState(), "a")
    if (!summary) throw new Error("no session a")
    const before = reads.length
    await act(async () =>
      fake.emit({
        kind: "session",
        session: { ...summary, ask: ask("live"), revision: summary.revision + 1 },
      }),
    )
    await settle()
    await frames()
    const line = host.querySelector('[data-owner-ask="live"] [data-auto-pick-line]')
    expect(line?.textContent).toContain("Picking Ship…")
    // One for the ask's change, one because the pick is late.
    expect(reads.length).toBeGreaterThanOrEqual(before + 2)
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
