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
  openSession,
  sendMessage,
  setComposerText,
} from "../../adapters/store/commands"
import { workspaceActions } from "../../adapters/store/slice"
import { emptyTranscript, type Transcript } from "../../model/transcript"
import { ClockProvider } from "../../../../clock"
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
  it("shows a staged pick at once and sends on it once the source takes it", async () => {
    const { store, source } = await conversation()
    expect(shownModel()).toContain("Claude Opus 5")
    const mounted = field()
    await act(async () => {
      store.dispatch(chooseModel({ sessionId: "a", model: astra }))
    })
    expect(field()).toBe(mounted)
    expect(shownModel()).toContain("GPT-6 Astra")
    expect(host.querySelector(".desktop-chip-model")?.getAttribute("disabled")).toBeNull()
    expect(source.calls.some((call) => call[0] === "setModel")).toBe(false)
    expect(await sentWith(store, source, "a")).toEqual(astra)
    const order = source.calls.map((call) => call[0])
    expect(order.indexOf("setModel")).toBeLessThan(order.indexOf("send"))
  })

  it("takes a staged pick back when the session's own model is picked again", async () => {
    const { store, source } = await conversation()
    await act(async () => {
      store.dispatch(chooseModel({ sessionId: "a", model: astra }))
    })
    await act(async () => {
      store.dispatch(
        chooseModel({
          sessionId: "a",
          model: store.getState().workspace.sessions.a.model,
        }),
      )
    })
    expect(shownModel()).toContain("Claude Opus 5")
    expect(await sentWith(store, source, "a")).toEqual(
      store.getState().workspace.sessions.a.model,
    )
    expect(source.calls.some((call) => call[0] === "setModel")).toBe(false)
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

  it("says the chat cannot be reopened when the model change was refused as a configuration change", async () => {
    const { store } = await conversation()
    const choice = { requestId: "r", model: astra }
    await act(async () => {
      store.dispatch(
        workspaceActions.modelChoiceAsked({
          sessionId: "a",
          choice: { ...choice, state: "asking" },
        }),
      )
      store.dispatch(
        workspaceActions.modelChoiceSettled({
          sessionId: "a",
          choice: {
            ...choice,
            state: "recovery_required",
            reason: "unavailable",
            code: "conversation_configuration_changed",
          },
        }),
      )
    })
    expect(host.querySelector(".desktop-composer-notice")?.textContent).toBe(
      "This chat can't be reopened.",
    )
    await act(async () => {
      store.dispatch(
        workspaceActions.modelChoiceSettled({
          sessionId: "a",
          choice: {
            ...choice,
            state: "refused",
            reason: "not-supported",
            code: "conversation_configuration_changed",
          },
        }),
      )
    })
    expect(host.querySelector(".desktop-composer-notice")?.textContent).toBe(
      "This chat can't be reopened.",
    )
    await act(async () => {
      store.dispatch(
        workspaceActions.modelChoiceSettled({
          sessionId: "a",
          choice: { ...choice, state: "recovery_required", reason: "unavailable" },
        }),
      )
    })
    expect(host.querySelector(".desktop-composer-notice")?.textContent).toBe(
      "The model change is not confirmed. Send again to check and try.",
    )
  })
  it("asks for no send while the source settles a model change it holds", async () => {
    const { store } = await conversation()
    await act(async () => {
      store.dispatch(
        workspaceActions.transcriptLoaded({
          transcript: {
            sessionId: "a",
            messages: [],
            activity: null,
            approvals: [],
            revision: 50,
            modelControl: {
              pending: {
                requestId: "elsewhere",
                requestedModel: astra.modelId,
                status: "recovery_required",
              },
            },
          },
        }),
      )
      store.dispatch(setComposerText({ sessionId: "a", text: "next" }))
    })
    expect(host.querySelector(".desktop-composer-notice")?.textContent).toBe(
      "The model change is not confirmed yet. You can send once it settles.",
    )
    expect(
      host.querySelector<HTMLButtonElement>('button[aria-label="Send"]')?.disabled,
    ).toBe(true)
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

describe("a pane's composer offers Stop while the agent is at work", () => {
  const button = () => host.querySelector<HTMLButtonElement>(".desktop-send")
  const atWork = (change: Partial<Transcript>, revision = 2): Transcript => ({
    ...emptyTranscript("a"),
    revision,
    ...change,
  })
  async function show(source: FakeSource, transcript: Transcript) {
    await act(async () => {
      source.transcripts.set("a", transcript)
      source.emit({ kind: "transcript", transcript })
      await settle()
    })
  }

  it("turns Send into Stop for a running turn, asks the source once, and turns back when it ends", async () => {
    const { store, source } = await conversation()
    await act(async () => {
      store.dispatch(openSession({ sessionId: "a" }))
    })
    expect(button()?.getAttribute("aria-label")).toBe("Send")
    await show(source, atWork({ activity: { label: "Running cargo test", since: 1 } }))
    expect(button()?.getAttribute("aria-label")).toBe("Stop")
    // Stop needs no draft: it is pressable with the field empty.
    expect(button()?.disabled).toBe(false)
    await act(async () => {
      button()?.click()
      await settle()
    })
    expect(source.calls.filter((call) => call[0] === "stop")).toEqual([
      ["stop", "a", "person"],
    ])
    expect(button()?.getAttribute("aria-label")).toBe("Send")
  })

  it("offers Stop when the view left a review out and the turn still runs", async () => {
    const { store, source } = await conversation()
    await act(async () => {
      store.dispatch(openSession({ sessionId: "a" }))
    })
    // The gateway's view past its review limit: no approval shown, its
    // notice instead, and the turn's reply still being written.
    await show(
      source,
      atWork({
        interactionNotice:
          "A pending tool review exceeds the display limit; no choices were silently removed.",
        history: { omitted: true, unconfirmed: false },
        messages: [
          { id: "in", role: "user", at: 1, parts: [{ kind: "text", text: "Go" }] },
          {
            id: "out",
            role: "agent",
            at: 2,
            working: true,
            parts: [{ kind: "step", step: "run", label: "cargo test" }],
          },
        ],
      }),
    )
    expect(button()?.getAttribute("aria-label")).toBe("Stop")
  })
})
