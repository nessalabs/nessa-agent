// @vitest-environment jsdom
/**
 * A pane's composer shows the model its next message is sent with, whoever
 * changed it: the workspace's `modelForNextTurn` is the one owner, and the
 * picker renders from it. An agent's `chooseModel` and a newer summary from
 * the source each change what the picker shows while the composer stays
 * mounted, and the next message goes with what was shown.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import {
  chooseModel,
  followWorkspace,
  loadWorkspace,
  newSession,
  sendMessage,
} from "../../adapters/store/commands"
import { ClockProvider } from "../../adapters/dom/clock"
import type { ModelRef } from "../../model/workspace-index"
import {
  astra,
  fakeSource,
  settle,
  summary,
  testStore,
  type FakeSource,
} from "../../testing"
import { Conversation, PaneHome } from "./conversation"

class Observer {
  observe() {}
  unobserve() {}
  disconnect() {}
}

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer })
  Element.prototype.scrollTo ??= () => {}
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
})

type Store = ReturnType<typeof testStore>

async function mount(store: Store, content: React.ReactNode) {
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1000}>{content}</ClockProvider>
      </Provider>,
    ),
  )
}

async function conversation(source: FakeSource = fakeSource()) {
  const store = testStore(source)
  store.dispatch(followWorkspace())
  await store.dispatch(loadWorkspace())
  await settle()
  await mount(store, <Conversation sessionId="a" onHeadingVisible={() => {}} />)
  return { store, source }
}

/** The model the composer's picker shows. */
const shownModel = () => host.querySelector(".desktop-chip-model")?.textContent ?? ""
const field = () => host.querySelector('textarea[aria-label="Message"]')

/** The model the source was asked to send the last message with. */
async function sentWith(store: Store, source: FakeSource, sessionId: string) {
  await act(async () => {
    await store.dispatch(sendMessage({ sessionId, text: "next", initiator: "person" }))
  })
  const sends = source.calls.filter((call) => call[0] === "send")
  return (sends.at(-1)?.[1] as { model?: ModelRef } | undefined)?.model
}

describe("a pane's composer shows the model the next message is sent with", () => {
  it("follows a model an agent chooses, with the composer still mounted", async () => {
    const { store, source } = await conversation()
    expect(shownModel()).toContain("Claude Opus 5")
    const mounted = field()
    await act(async () => {
      store.dispatch(chooseModel({ sessionId: "a", model: astra }))
    })
    expect(field()).toBe(mounted)
    expect(shownModel()).toContain("GPT-6 Astra")
    expect(await sentWith(store, source, "a")).toEqual(astra)
  })

  it("follows a newer summary that moves the session to another model", async () => {
    const { store, source } = await conversation()
    const mounted = field()
    await act(async () => {
      source.emit({
        kind: "session",
        session: summary("a", "desktop", 500, "idle", { model: astra, revision: 2 }),
      })
      await settle()
    })
    expect(field()).toBe(mounted)
    expect(shownModel()).toContain("GPT-6 Astra")
    expect(await sentWith(store, source, "a")).toEqual(astra)
  })

  it("follows a model chosen for a new session's home", async () => {
    const source = fakeSource()
    const store = testStore(source)
    await store.dispatch(loadWorkspace())
    await settle()
    const draftId = store.dispatch(newSession())
    if (!draftId) throw new Error("no draft")
    await mount(store, <PaneHome sessionId={draftId} onSend={() => {}} />)
    expect(shownModel()).toContain("Claude Opus 5")
    await act(async () => {
      store.dispatch(chooseModel({ sessionId: draftId, model: astra }))
    })
    expect(shownModel()).toContain("GPT-6 Astra")
    expect(await sentWith(store, source, draftId)).toEqual(astra)
  })
})

describe("a new session's home in a pane", () => {
  it("holds its one composer in the dock's box, the box a conversation's composer sits in", async () => {
    const store = testStore()
    await store.dispatch(loadWorkspace())
    await settle()
    const draftId = store.dispatch(newSession())
    if (!draftId) throw new Error("no draft")
    await mount(store, <PaneHome sessionId={draftId} onSend={() => {}} />)
    // Which shape it takes is the pane's container query (`conversation.css`),
    // which a DOM without layout cannot run: where a small pane docks this box,
    // and that the draft and caret survive, `responsive.mjs --only home-shape`
    // measures in a browser.
    expect(host.querySelectorAll(".desktop-composer")).toHaveLength(1)
    expect(
      host.querySelector(".desktop-home .workspace-dock > .desktop-composer"),
    ).not.toBeNull()
  })
})

describe("a new session's home greets by the hour of the window's clock", () => {
  const at = (hour: number) => new Date(2026, 8, 29, hour, 0).getTime()
  const greeting = () => host.querySelector(".desktop-greeting")?.textContent

  it("says good morning in the morning, and asks if it is late only at night", async () => {
    const store = testStore(fakeSource())
    await store.dispatch(loadWorkspace())
    for (const [hour, said] of [
      [9, "Good morning"],
      [23, "Working late?"],
    ] as const) {
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ClockProvider now={() => at(hour)}>
              <PaneHome sessionId="a" onSend={() => {}} />
            </ClockProvider>
          </Provider>,
        ),
      )
      expect(greeting()).toBe(said)
    }
  })
})
