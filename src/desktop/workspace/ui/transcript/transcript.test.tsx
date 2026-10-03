// @vitest-environment jsdom
/**
 * A conversation as drawn: each kind of part, a message the source refused,
 * and an approval that asks once and says why an answer did not arrive.
 */
import { act, StrictMode, createRef } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { loadWorkspace, openSession, sendMessage } from "../../adapters/store/commands"
import { workspaceActions } from "../../adapters/store/slice"
import { ClockProvider } from "../../adapters/dom/clock"
import {
  emptyTranscript,
  type Transcript as TranscriptValue,
} from "../../model/transcript"
import { fakeSource, settle, testStore } from "../../testing"
import { Transcript } from "./transcript"

class Observer {
  observe() {}
  disconnect() {}
}

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer })
  Element.prototype.scrollTo ??= () => {}
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const conversation: TranscriptValue = {
  ...emptyTranscript("b"),
  revision: 1,
  messages: [
    {
      id: "m1",
      role: "user",
      at: 1,
      parts: [{ kind: "text", text: "Why `cargo` fails?" }],
    },
    {
      id: "m2",
      role: "agent",
      at: 2,
      parts: [
        { kind: "step", step: "read", label: "Read", detail: "Cargo.toml" },
        {
          kind: "step",
          step: "edit",
          label: "Edited",
          detail: "lib.rs",
          added: 3,
          removed: 1,
        },
        { kind: "text", text: "It is **the lockfile**." },
        { kind: "code", code: "cargo update" },
        { kind: "list", items: ["one", "two"] },
      ],
    },
  ],
  approval: { id: "ap", command: "cargo test", reason: "Runs the tests." },
}

async function shown(source = fakeSource(), transcript = conversation) {
  source.transcripts.set("b", transcript)
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  store.dispatch(openSession({ sessionId: "b" }))
  await settle()
  await act(async () => {
    root.render(
      <StrictMode>
        <Provider store={store}>
          <ClockProvider now={() => 1000}>
            <Transcript
              sessionId="b"
              arriving={false}
              scrollRef={createRef()}
              headingRef={createRef()}
              onHeadingVisible={() => {}}
            />
          </ClockProvider>
        </Provider>
      </StrictMode>,
    )
  })
  return { store, source }
}

const buttons = () =>
  [...host.querySelectorAll<HTMLButtonElement>(".workspace-approval button")].map(
    (button) => [
      button.getAttribute("aria-label") ?? button.textContent,
      button.disabled,
    ],
  )

describe("a transcript", () => {
  it("draws prose, steps, code and lists, and says when a message was not sent", async () => {
    const source = fakeSource()
    source.refuse("send", "unavailable")
    const { store } = await shown(source)
    await act(async () => {
      await store.dispatch(
        sendMessage({ initiator: "person", sessionId: "b", text: "Try again" }),
      )
    })
    expect(host.querySelector(".workspace-heading h2")?.textContent).toBe("Session b")
    expect(host.querySelector(".workspace-bubble code")?.textContent).toBe("cargo")
    expect(
      [...host.querySelectorAll(".workspace-steps li")].map((step) => step.textContent),
    ).toEqual(["ReadCargo.toml", "Editedlib.rs+3−1"])
    expect(host.querySelector(".workspace-message-body strong")?.textContent).toBe(
      "the lockfile",
    )
    expect(host.querySelector(".workspace-code")?.textContent).toBe("cargo update")
    expect(host.querySelectorAll(".workspace-list-items li")).toHaveLength(2)
    const failed = host.querySelector(".workspace-message-failed")
    expect(failed?.querySelector("span")?.textContent).toBe(
      "Not sent. Nessa couldn’t confirm this just now.",
    )
    expect(
      [...(failed?.querySelectorAll("button") ?? [])].map((b) => b.textContent),
    ).toEqual(["Send Again", "Discard"])
  })

  it("says above a message of the person's which app wrote it, and nothing above their own", async () => {
    await shown()
    expect(host.querySelector(".workspace-message-author")).toBeNull()
    await act(async () => root.unmount())
    root = createRoot(host)
    const written: TranscriptValue = {
      ...conversation,
      revision: 2,
      messages: [
        ...conversation.messages,
        {
          id: "m3",
          role: "user",
          at: 3,
          parts: [{ kind: "text", text: "Plot May" }],
          app: { server: "charts", tool: "show" },
        },
      ],
    }
    await shown(fakeSource(), written)
    const authors = [...host.querySelectorAll(".workspace-message-author")]
    // Named as the app's own view names it: its tool, from its server.
    expect(authors.map((author) => author.textContent)).toEqual([
      "Sent by show, from charts",
    ])
    expect(authors[0]?.getAttribute("title")).toBe("Sent by show, from charts")
    expect(authors[0]?.closest(".workspace-message")?.getAttribute("data-role")).toBe(
      "user",
    )
  })

  it("keeps messages sent while the conversation was read in place when it loads", async () => {
    const source = fakeSource()
    source.transcripts.set("b", conversation)
    const store = testStore(source)
    await store.dispatch(loadWorkspace())
    await settle()
    source.hold("transcript")
    store.dispatch(openSession({ sessionId: "b" }))
    await act(async () => {
      root.render(
        <Provider store={store}>
          <ClockProvider now={() => 1000}>
            <Transcript
              sessionId="b"
              arriving
              scrollRef={createRef()}
              headingRef={createRef()}
              onHeadingVisible={() => {}}
            />
          </ClockProvider>
        </Provider>,
      )
    })
    await act(async () => {
      void store.dispatch(
        sendMessage({ initiator: "person", sessionId: "b", text: "one" }),
      )
      void store.dispatch(
        sendMessage({ initiator: "person", sessionId: "b", text: "two" }),
      )
      await settle()
    })
    await act(async () => {
      await source.release("transcript")
      await settle()
    })
    const rising = [...host.querySelectorAll(".workspace-message[data-new]")].map(
      (message) => message.textContent,
    )
    // What the source held and was never on screen may rise; what was shown stays put.
    expect(rising).not.toContain("one")
    expect(rising).not.toContain("two")
    // Arriving, what the source says beyond its first message was never on screen: it rises.
    expect(rising.length).toBeGreaterThan(0)
    // A reply the source places before a message still pending rises all the same.
    const [one] = store.getState().workspace.outbox.b ?? []
    const reply = {
      id: "reply",
      role: "agent" as const,
      at: 2000,
      parts: [{ kind: "text" as const, text: "On it." }],
    }
    await act(async () => {
      store.dispatch(
        workspaceActions.updateReceived({
          update: {
            kind: "transcript",
            transcript: {
              ...conversation,
              revision: conversation.revision + 1,
              messages: [
                ...conversation.messages,
                { ...one, delivery: undefined },
                reply,
              ],
            },
          },
        }),
      )
    })
    const risen = [...host.querySelectorAll(".workspace-message[data-new]")].map(
      (message) => message.textContent,
    )
    expect(risen).toContain("On it.")
    expect(risen).not.toContain("one")
    expect(risen).not.toContain("two")
  })

  it("sends a refused message again, or lets it go, from beneath it", async () => {
    const source = fakeSource()
    source.refuse("send", "unavailable")
    const { store } = await shown(source)
    await act(async () => {
      await store.dispatch(
        sendMessage({ initiator: "person", sessionId: "b", text: "one" }),
      )
    })
    const unsent = (label: string) =>
      [
        ...host.querySelectorAll<HTMLButtonElement>(".workspace-message-failed button"),
      ].find((button) => button.textContent === label)
    await act(async () => {
      unsent("Send Again")?.click()
      await settle()
    })
    expect(source.calls.filter((call) => call[0] === "send")).toHaveLength(2)
    expect(host.querySelectorAll(".workspace-message-failed")).toHaveLength(1)
    await act(async () => {
      unsent("Discard")?.click()
    })
    expect(host.querySelector(".workspace-message-failed")).toBeNull()
    expect(store.getState().workspace.outbox.b).toBeUndefined()
  })

  it("answers an approval once, resting its buttons while the answer is on its way", async () => {
    const source = fakeSource()
    source.hold("approve")
    await shown(source)
    // Every answer, whichever of them the card's width shows (`approval-request.tsx`).
    expect(buttons()).toEqual([
      ["Deny", false],
      ["Always Allow", false],
      ["Allow Once", false],
      ["More Ways to Allow", false],
    ])
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".workspace-approval [data-primary]")?.click()
    })
    expect(buttons().every(([, disabled]) => disabled)).toBe(true)
    expect(source.calls.filter((call) => call[0] === "approve")).toEqual([
      ["approve", "b", "ap", "once", "person"],
    ])
  })

  it("asks again, saying why, when an answer does not reach the agent", async () => {
    const source = fakeSource()
    source.refuse("deny", "not-waiting")
    await shown(source)
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".workspace-approval button")?.click()
    })
    await act(async () => settle())
    expect(host.querySelector(".workspace-approval-failure")?.textContent).toBe(
      "This was already answered.",
    )
    expect(buttons().every(([, disabled]) => !disabled)).toBe(true)
  })

  it("shows what the agent is doing, and nothing once its reply streams", async () => {
    const { store } = await shown()
    await act(async () => {
      store.dispatch(
        workspaceActions.updateReceived({
          update: {
            kind: "transcript",
            transcript: {
              ...conversation,
              revision: 2,
              activity: { label: "Thinking", since: 0 },
            },
          },
        }),
      )
    })
    expect(host.querySelector(".workspace-live")?.textContent).toBe("Thinking1s")
    await act(async () => {
      store.dispatch(
        workspaceActions.updateReceived({
          update: {
            kind: "transcript",
            transcript: { ...conversation, revision: 3, activity: null },
          },
        }),
      )
    })
    expect(host.querySelector(".workspace-live")).toBeNull()
  })
})
