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
import { transcriptFrom } from "../../adapters/gateway/gateway-views"
import { view } from "../../adapters/gateway/fake-gateway"
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
  approval: {
    id: "ap",
    command: "cargo test",
    reason: "Runs the tests.",
    origin: { kind: "agent" },
  },
}

async function shown(source = fakeSource(), shownConversation = conversation) {
  source.transcripts.set("b", shownConversation)
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

  it("O1, O2 (#436): names who asks — the agent by its name, an app by its server and the tool it named, never the agent for an app", async () => {
    await shown()
    const head = () => host.querySelector(".workspace-approval-head")?.textContent ?? ""
    const card = () => host.querySelector<HTMLElement>(".workspace-approval")
    expect(head()).toMatch(/^\S.* wants to run a command$/)
    expect(card()?.dataset.origin).toBe("agent")
    const agent = head().replace(/ wants to run a command$/, "")
    await act(async () => root.render(<></>))
    await shown(fakeSource(), {
      ...conversation,
      approval: {
        id: "app-ap",
        command: "app_delete_row {}",
        reason: "An app asks to run app_delete_row on mcptest",
        origin: { kind: "app", server: "mcptest", tool: "app_delete_row" },
      },
    })
    expect(head()).toBe("The mcptest app wants to run app_delete_row")
    expect(head()).not.toContain(agent)
    expect(card()?.dataset.origin).toBe("app")
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

describe("provider recovery ownership", () => {
  it("retires recovery while a newer user-only message is sending or failed", async () => {
    const source = fakeSource()
    source.hold("send")
    const { store } = await shown(source, {
      ...emptyTranscript("b"),
      revision: 1,
      authenticationRequired: true,
      messages: [conversation.messages[0]],
    })
    expect(host.querySelector(".provider-sign-in")).not.toBeNull()
    let sent: Promise<unknown> | undefined
    await act(async () => {
      sent = store.dispatch(
        sendMessage({ initiator: "person", sessionId: "b", text: "Try again" }),
      )
      await settle()
    })
    expect(host.textContent).toContain("Try again")
    expect(host.querySelector(".provider-sign-in")).toBeNull()
    source.refuse("send", "unavailable")
    await act(async () => {
      await source.release("send")
      await sent
    })
    expect(host.querySelector(".workspace-message-failed")).not.toBeNull()
    expect(host.querySelector(".provider-sign-in")).toBeNull()
    const [retry] = store.getState().workspace.outbox.b ?? []
    const accepted = view("b", {
      messages: [
        {
          executionId: "m1",
          userText: "Why cargo fails?",
          attachments: [],
          files: [],
          status: "failed",
          authenticationRequired: true,
          parts: [],
        },
      ],
      pending: [
        {
          executionId: retry.id,
          text: "Try again",
          attachments: [],
          files: [],
          mode: "queued",
        },
      ],
    })
    for (const phase of ["queued", "running", "completed"] as const) {
      if (phase !== "queued") {
        accepted.pending = []
        accepted.messages[1] = {
          executionId: retry.id,
          userText: "Try again",
          attachments: [],
          files: [],
          status: phase,
          parts:
            phase === "completed"
              ? [{ kind: "text", offset: 0, text: "Ready", toolId: "", noticeId: "" }]
              : [],
        }
      }
      await act(async () => {
        store.dispatch(
          workspaceActions.updateReceived({
            update: {
              kind: "transcript",
              transcript: transcriptFrom(
                accepted,
                phase === "queued" ? 2 : phase === "running" ? 3 : 4,
                () => 1000,
              ),
            },
          }),
        )
      })
      expect(store.getState().workspace.outbox.b).toBeUndefined()
      expect(host.querySelector(".provider-sign-in")).toBeNull()
      expect(
        [...host.querySelectorAll(".workspace-message")].filter(
          (message) => message.textContent === "Try again",
        ),
      ).toHaveLength(1)
    }
  })
})
