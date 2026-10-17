import { WorkspaceSendFailure } from "../send-failure"
// @vitest-environment jsdom
/**
 * A conversation as drawn: each kind of part, a message the source refused,
 * and an approval that asks once and says why an answer did not arrive.
 */
import { act, StrictMode, createRef } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  answerQuestion,
  loadWorkspace,
  followWorkspace,
  openSession,
  sendMessage,
  resendMessage,
} from "../../adapters/store/commands"
import { transcriptFrom } from "../../adapters/gateway/gateway-views"
import { GATEWAY_NOW_MS, view } from "../../adapters/gateway/fake-gateway"
import { workspaceActions } from "../../adapters/store/slice"
import { ClockProvider } from "../../../../clock"
import {
  emptyTranscript,
  type Transcript as TranscriptValue,
} from "../../model/transcript"
import { fakeSource, settle, testStore } from "../../testing"
import { failureCopy, readFailureCopy } from "../failure-copy"
import { approvalHead, approvalReason } from "./approval-request"
import { spoken } from "./said"
import { QuestionCard } from "./question-card"
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
    options: [
      { id: "deny", label: "Deny", choice: "deny" },
      { id: "always", label: "Always Allow", choice: "always" },
      { id: "once", label: "Allow Once", choice: "once" },
    ],
    ask: "tool",
  },
}

async function shown(
  source = fakeSource(),
  shownConversation = conversation,
  onHeadingVisible: (visible: boolean) => void = () => {},
) {
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
              scrollRef={createRef()}
              onHeadingVisible={onHeadingVisible}
            />
            <WorkspaceSendFailure sessionId="b" />
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

const unreadConversation = "Nessa couldn’t read this conversation just now."

async function failedRead(reason: "unavailable" | "signed-out") {
  const source = fakeSource()
  source.refuse("transcript", reason)
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  store.dispatch(openSession({ sessionId: "b" }))
  await settle()
  await act(async () => {
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1000}>
          <Transcript sessionId="b" scrollRef={createRef()} onHeadingVisible={() => {}} />
        </ClockProvider>
      </Provider>,
    )
  })
  return source
}

describe("a conversation that cannot be read", () => {
  it("says what it could not read, not that a call is unconfirmed, and reads it again", async () => {
    const source = await failedRead("unavailable")
    const note = host.querySelector(".workspace-transcript-note")
    expect(note?.querySelector("p")?.textContent).toBe(unreadConversation)
    expect(note?.querySelector("p")?.textContent).toBe(
      readFailureCopy("unavailable", "conversation"),
    )
    expect(note?.querySelector("p")?.textContent).not.toBe(failureCopy("unavailable"))
    expect(note?.querySelector("button")?.textContent).toBe("Try Again")
    source.refuse("transcript", undefined)
    const asked = source.calls.filter(
      (call) => call[0] === "transcript" && call[1] === "b",
    ).length
    await act(async () => {
      note?.querySelector("button")?.click()
      await settle()
    })
    expect(
      source.calls.filter((call) => call[0] === "transcript" && call[1] === "b"),
    ).toHaveLength(asked + 1)
    expect(host.querySelector(".workspace-transcript-note")).toBeNull()
  })

  it("says the window is signed out in the same words as any call", async () => {
    await failedRead("signed-out")
    const said = host.querySelector(".workspace-transcript-note p")?.textContent
    expect(said).toBe("This window isn’t signed in to the local server.")
    expect(said).toBe(readFailureCopy("signed-out", "conversation"))
  })
})

describe("a bounded conversation", () => {
  const notices = () =>
    [...host.querySelectorAll("[data-history-extent]")].map(
      (notice) => notice.textContent,
    )

  it("says at once when the view left history out, and when that history is not confirmed", async () => {
    await shown(fakeSource(), {
      ...conversation,
      history: { omitted: true, unconfirmed: true },
    })
    expect(notices()).toEqual([
      "Some conversation content is not shown.",
      "Conversation history is not confirmed. Recent activity may be missing.",
    ])
    expect(host.querySelector(".workspace-bubble")?.textContent).toContain("cargo")
  })

  it("says nothing of history the view says is whole", async () => {
    await shown()
    expect(notices()).toEqual([])
  })

  it("replaces the notice and the messages when a later view is the whole conversation", async () => {
    const source = fakeSource()
    const { store } = await shown(source, {
      ...conversation,
      revision: 4,
      history: { omitted: true, unconfirmed: false },
    })
    expect(notices()).toEqual(["Some conversation content is not shown."])
    store.dispatch(followWorkspace())
    const next: TranscriptValue = {
      ...conversation,
      revision: 5,
      messages: [
        ...conversation.messages,
        {
          id: "m3",
          role: "agent",
          at: 3,
          parts: [{ kind: "text", text: "The rest of it." }],
        },
      ],
    }
    await act(async () => {
      source.emit({ kind: "transcript", transcript: next })
      await settle()
    })
    expect(notices()).toEqual([])
    const texts = [...host.querySelectorAll(".workspace-message")].map(
      (message) => message.textContent,
    )
    expect(texts).toHaveLength(3)
    expect(texts.filter((text) => text?.includes("Why cargo fails?"))).toHaveLength(1)
    expect(texts.filter((text) => text?.includes("The rest of it."))).toHaveLength(1)
  })
})

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
    // The turn has ended, so its steps are one line until opened.
    const work = host.querySelector<HTMLButtonElement>(".workspace-work button")
    expect(work?.textContent).toBe("Ran 2 tools · 1 read, 1 edited")
    expect(host.querySelector(".workspace-steps")).toBeNull()
    await act(async () => work?.click())
    expect(
      [...host.querySelectorAll(".workspace-steps li")].map((step) => step.textContent),
    ).toEqual(["ReadCargo.toml", "Editedlib.rs+3−1"])
    expect(host.querySelector(".workspace-message-body strong")?.textContent).toBe(
      "the lockfile",
    )
    expect(host.querySelector(".workspace-code")?.textContent).toBe("cargo update")
    expect(host.querySelectorAll(".workspace-list-items li")).toHaveLength(2)
    const failed = host.querySelector("[data-send-failure]")
    expect(failed?.querySelector("span")?.textContent).toBe("Error sending")
    expect(
      [...(failed?.querySelectorAll("button") ?? [])]
        .filter((b) => b.textContent)
        .map((b) => b.textContent),
    ).toEqual(["Send Again", "Discard"])
  })

  it("shows a failed turn's status and diagnostic apart from a message that was not sent", async () => {
    await shown(fakeSource(), {
      ...conversation,
      messages: [
        {
          id: "m1",
          role: "user",
          at: 1,
          parts: [{ kind: "text", text: "Check the deploy." }],
          terminal: { status: "failed", error: "Provider connection lost" },
        },
        {
          id: "m2",
          role: "agent",
          at: 2,
          parts: [{ kind: "text", text: "Halfway" }],
          terminal: { status: "cancelled", error: "Stopped" },
        },
        {
          id: "m3",
          role: "user",
          at: 3,
          parts: [{ kind: "text", text: "Wait." }],
          terminal: { status: "unresolved" },
        },
      ],
      approval: null,
    })
    const marks = [...host.querySelectorAll<HTMLElement>("[data-turn-terminal]")]
    expect(marks.map((mark) => mark.dataset.turnTerminal)).toEqual([
      "failed",
      "cancelled",
      "unresolved",
    ])
    expect(marks.map((mark) => mark.dataset.turnError ?? null)).toEqual([
      "Provider connection lost",
      "Stopped",
      null,
    ])
    expect(marks.map((mark) => mark.querySelector("span")?.textContent)).toEqual([
      "Failed",
      "Cancelled",
      "Unresolved",
    ])
    expect(host.textContent).toContain("Check the deploy.")
    expect(host.textContent).toContain("Halfway")
    expect(host.querySelector("[data-send-failure]")).toBeNull()
  })

  it("shows the gateway's interaction notice in its own words", async () => {
    await shown(fakeSource(), {
      ...conversation,
      approval: null,
      interactionNotice: "A review was left out of this view.",
    })
    expect(host.querySelector("[data-interaction-notice]")?.textContent).toBe(
      "A review was left out of this view.",
    )
  })

  it("answers a question the conversation is showing and leaves one it is not", async () => {
    const source = fakeSource()
    const { store } = await shown(source, {
      ...conversation,
      approval: null,
      questions: [
        {
          executionId: "e1",
          questionId: "place",
          message: "Where should this land?",
          questions: [
            {
              key: "where",
              prompt: "Pick a place",
              multiSelect: false,
              freeText: true,
              required: true,
              options: [{ value: "here", label: "Here" }],
            },
          ],
        },
      ],
    })
    store.dispatch(followWorkspace())
    const card = () => host.querySelector<HTMLElement>('[data-question="place"]')
    const button = (label: string) =>
      [...(card()?.querySelectorAll("button") ?? [])].find(
        (candidate) => candidate.textContent === label,
      )
    expect(button("Answer")?.disabled).toBe(true)
    const words = card()?.querySelector<HTMLInputElement>('input[name="where_custom"]')
    const setValue = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set
    await act(async () => {
      setValue?.call(words, "in my own words")
      words?.dispatchEvent(new Event("input", { bubbles: true }))
    })
    expect(button("Answer")?.disabled).toBe(true)
    const choice = card()?.querySelector<HTMLInputElement>('input[type="radio"]')
    if (!choice) throw new Error("the question offers no choice")
    await act(async () => choice.click())
    expect(button("Answer")?.disabled).toBe(false)
    await act(async () => {
      button("Answer")?.click()
      await settle()
    })
    expect(source.calls.find((call) => call[0] === "answerQuestion")).toEqual([
      "answerQuestion",
      "b",
      "e1",
      "place",
      [{ key: "where", values: ["here"], ownWords: "in my own words" }],
      "person",
    ])
    expect(host.querySelector('[data-question="place"]')).toBeNull()
    const missed = await store.dispatch(
      answerQuestion({
        sessionId: "b",
        executionId: "e1",
        questionId: "place",
        choices: null,
        initiator: "person",
      }),
    )
    expect(missed).toEqual({ status: "not-asked" })
    expect(source.calls.filter((call) => call[0] === "answerQuestion")).toHaveLength(1)
    expect(button("Skip")).toBeUndefined()
  })

  it("does not send a question again while the first answer is on its way", async () => {
    const source = fakeSource()
    source.hold("answerQuestion")
    const { store } = await shown(source, {
      ...conversation,
      approval: null,
      questions: [
        {
          executionId: "e1",
          questionId: "place",
          message: "Where should this land?",
          questions: [
            {
              key: "where",
              prompt: "Pick a place",
              multiSelect: false,
              freeText: false,
              required: false,
              options: [{ value: "here", label: "Here" }],
            },
          ],
        },
      ],
    })
    const ask = {
      sessionId: "b",
      executionId: "e1",
      questionId: "place",
      choices: null,
      initiator: "person" as const,
    }
    const first = store.dispatch(answerQuestion(ask))
    const second = store.dispatch(answerQuestion(ask))
    await settle()
    expect(source.calls.filter((call) => call[0] === "answerQuestion")).toHaveLength(1)
    await source.release("answerQuestion")
    expect(await first).toEqual({ status: "sent" })
    expect(await second).toEqual({ status: "sent" })
  })

  it("does not report a different answer as sent while one is already on its way", async () => {
    const source = fakeSource()
    source.hold("answerQuestion")
    const { store } = await shown(source, {
      ...conversation,
      approval: null,
      questions: [
        {
          executionId: "e1",
          questionId: "place",
          message: "Where should this land?",
          questions: [
            {
              key: "where",
              prompt: "Pick a place",
              multiSelect: false,
              freeText: false,
              required: false,
              options: [{ value: "here", label: "Here" }],
            },
          ],
        },
      ],
    })
    const skip = {
      sessionId: "b",
      executionId: "e1",
      questionId: "place",
      choices: null,
      initiator: "person" as const,
    }
    const answer = {
      ...skip,
      choices: [{ key: "where", values: ["here"] }],
    }
    const first = store.dispatch(answerQuestion(skip))
    const otherInitiator = store.dispatch(answerQuestion({ ...skip, initiator: "agent" }))
    const otherChoice = store.dispatch(answerQuestion(answer))
    const same = store.dispatch(answerQuestion(skip))
    const settled = { otherInitiator: false, otherChoice: false }
    void otherInitiator.then(() => {
      settled.otherInitiator = true
    })
    void otherChoice.then(() => {
      settled.otherChoice = true
    })
    await settle()
    expect(settled).toEqual({ otherInitiator: true, otherChoice: true })
    expect(await otherInitiator).toEqual({ status: "different" })
    expect(await otherChoice).toEqual({ status: "different" })
    expect(source.calls.filter((call) => call[0] === "answerQuestion")).toEqual([
      ["answerQuestion", "b", "e1", "place", null, "person"],
    ])
    source.refuse("answerQuestion", "unavailable")
    await source.release("answerQuestion")
    expect(await first).toEqual({ status: "unknown", reason: "unavailable" })
    expect(await same).toEqual({ status: "unknown", reason: "unavailable" })
    source.refuse("answerQuestion", undefined)
    expect(await store.dispatch(answerQuestion(answer))).toEqual({ status: "sent" })
    expect(source.calls.filter((call) => call[0] === "answerQuestion")).toEqual([
      ["answerQuestion", "b", "e1", "place", null, "person"],
      [
        "answerQuestion",
        "b",
        "e1",
        "place",
        [{ key: "where", values: ["here"] }],
        "person",
      ],
    ])
  })

  it("keeps a card whose answer differs from the one already on its way", async () => {
    const source = fakeSource()
    source.hold("answerQuestion")
    const question = {
      executionId: "e1",
      questionId: "place",
      message: "Where should this land?",
      questions: [
        {
          key: "where",
          prompt: "Pick a place",
          multiSelect: false,
          freeText: false,
          required: false,
          options: [{ value: "here", label: "Here" }],
        },
      ],
    }
    const { store } = await shown(source, {
      ...conversation,
      approval: null,
      questions: [question],
    })
    await act(async () => {
      root.render(
        <StrictMode>
          <Provider store={store}>
            <QuestionCard sessionId="b" question={question} />
            <QuestionCard sessionId="b" question={question} />
          </Provider>
        </StrictMode>,
      )
    })
    const cards = () => [...host.querySelectorAll<HTMLElement>('[data-question="place"]')]
    const button = (card: HTMLElement | undefined, label: string) =>
      [...(card?.querySelectorAll("button") ?? [])].find(
        (candidate) => candidate.textContent === label,
      )
    const choice = cards()[0]?.querySelector<HTMLInputElement>('input[type="radio"]')
    if (!choice) throw new Error("the question offers no choice")
    await act(async () => {
      choice.click()
      button(cards()[0], "Answer")?.click()
      button(cards()[1], "Skip")?.click()
      await settle()
    })
    expect(source.calls.filter((call) => call[0] === "answerQuestion")).toEqual([
      [
        "answerQuestion",
        "b",
        "e1",
        "place",
        [{ key: "where", values: ["here"] }],
        "person",
      ],
    ])
    const drawn = cards()
    expect(drawn).toHaveLength(2)
    expect(drawn[0]?.getAttribute("aria-busy")).toBe("true")
    expect(button(drawn[0], "Answer")?.disabled).toBe(true)
    expect(drawn[1]?.getAttribute("aria-busy")).toBe("false")
    expect(button(drawn[1], "Skip")?.disabled).toBe(false)
    expect(drawn[1]?.querySelector(".workspace-question-failure")?.textContent).toBe(
      "A different answer is already on its way.",
    )
    expect(drawn[1]?.textContent).not.toContain("This was already answered.")
    await source.release("answerQuestion")
  })

  it("lets a second card retry after the send already on its way is refused", async () => {
    const source = fakeSource()
    source.hold("answerQuestion")
    const question = {
      executionId: "e1",
      questionId: "place",
      message: "Where should this land?",
      questions: [
        {
          key: "where",
          prompt: "Pick a place",
          multiSelect: false,
          freeText: false,
          required: false,
          options: [{ value: "here", label: "Here" }],
        },
      ],
    }
    const { store } = await shown(source, {
      ...conversation,
      approval: null,
      questions: [question],
    })
    await act(async () => {
      root.render(
        <StrictMode>
          <Provider store={store}>
            <QuestionCard sessionId="b" question={question} />
            <QuestionCard sessionId="b" question={question} />
          </Provider>
        </StrictMode>,
      )
    })
    const cards = () => [...host.querySelectorAll<HTMLElement>('[data-question="place"]')]
    const skip = (card: HTMLElement | undefined) =>
      [...(card?.querySelectorAll("button") ?? [])].find(
        (button) => button.textContent === "Skip",
      )
    expect(cards()).toHaveLength(2)
    await act(async () => {
      skip(cards()[0])?.click()
      skip(cards()[1])?.click()
    })
    expect(source.calls.filter((call) => call[0] === "answerQuestion")).toHaveLength(1)
    source.refuse("answerQuestion", "unavailable")
    await act(async () => {
      await source.release("answerQuestion")
      await settle()
    })
    const after = cards()
    expect(after).toHaveLength(2)
    for (const card of after) {
      expect(card.querySelector(".workspace-question-failure")?.textContent).toBe(
        failureCopy("unavailable"),
      )
      expect(skip(card)?.disabled).toBe(false)
    }
    source.refuse("answerQuestion", undefined)
    source.hold("answerQuestion")
    await act(async () => {
      skip(cards()[0])?.click()
      await settle()
    })
    expect(source.calls.filter((call) => call[0] === "answerQuestion")).toHaveLength(2)
    await act(async () => {
      await source.release("answerQuestion")
    })
  })

  it("does not share a question send across two stores", async () => {
    const held = fakeSource()
    const other = fakeSource()
    held.hold("answerQuestion")
    const ask = {
      sessionId: "b",
      executionId: "e1",
      questionId: "place",
      choices: null,
      initiator: "person" as const,
    }
    const question = {
      executionId: "e1",
      questionId: "place",
      message: "Where should this land?",
      questions: [
        {
          key: "where",
          prompt: "Pick a place",
          multiSelect: false,
          freeText: false,
          required: false,
          options: [{ value: "here", label: "Here" }],
        },
      ],
    }
    const open = async (source: ReturnType<typeof fakeSource>) => {
      source.transcripts.set("b", {
        ...conversation,
        approval: null,
        questions: [question],
      })
      const store = testStore(source)
      await store.dispatch(loadWorkspace())
      store.dispatch(openSession({ sessionId: "b" }))
      await settle()
      return store
    }
    const firstStore = await open(held)
    const secondStore = await open(other)
    const first = firstStore.dispatch(answerQuestion(ask))
    expect(await secondStore.dispatch(answerQuestion(ask))).toEqual({ status: "sent" })
    expect(other.calls.filter((call) => call[0] === "answerQuestion")).toHaveLength(1)
    expect(held.calls.filter((call) => call[0] === "answerQuestion")).toHaveLength(1)
    await held.release("answerQuestion")
    expect(await first).toEqual({ status: "sent" })
  })

  it("removes a question card the window is not showing, without saying it was answered", async () => {
    const source = fakeSource()
    const question = {
      executionId: "e1",
      questionId: "place",
      message: "Where should this land?",
      questions: [
        {
          key: "where",
          prompt: "Pick a place",
          multiSelect: false,
          freeText: false,
          required: false,
          options: [{ value: "here", label: "Here" }],
        },
      ],
    }
    const store = testStore(source)
    await store.dispatch(loadWorkspace())
    store.dispatch(
      workspaceActions.transcriptLoaded({
        transcript: { ...conversation, approval: null, questions: [question] },
      }),
    )
    await act(async () => {
      root.render(
        <Provider store={store}>
          <QuestionCard sessionId="b" question={question} />
        </Provider>,
      )
    })
    const skip = [...host.querySelectorAll("button")].find(
      (button) => button.textContent === "Skip",
    )
    await act(async () => {
      skip?.click()
      await settle()
    })
    expect(host.querySelector('[data-question="place"]')).toBeNull()
    expect(host.textContent).not.toContain("This was already answered.")
    expect(source.calls.filter((call) => call[0] === "answerQuestion")).toHaveLength(0)
  })

  it("says a question was already answered when the source refuses it", async () => {
    const source = fakeSource()
    source.refuse("answerQuestion", "not-waiting")
    await shown(source, {
      ...conversation,
      approval: null,
      questions: [
        {
          executionId: "e1",
          questionId: "place",
          message: "Where should this land?",
          questions: [
            {
              key: "where",
              prompt: "Pick a place",
              multiSelect: false,
              freeText: false,
              required: false,
              options: [{ value: "here", label: "Here" }],
            },
          ],
        },
      ],
    })
    const skip = [...host.querySelectorAll("button")].find(
      (button) => button.textContent === "Skip",
    )
    await act(async () => {
      skip?.click()
      await settle()
    })
    expect(host.querySelector(".workspace-question-failure")?.textContent).toBe(
      "This was already answered.",
    )
    expect(source.calls.find((call) => call[0] === "answerQuestion")?.[4]).toBeNull()
  })

  it("D17 (#390): says above a message of the person's which app wrote it, and nothing above their own", async () => {
    await shown(fakeSource(), {
      ...conversation,
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
    })
    const authors = [...host.querySelectorAll<HTMLElement>(".workspace-message-author")]
    // Named as the app's own view names it: its tool, from its server; once.
    expect(authors.map((author) => author.textContent)).toEqual([
      "Sent by show, from charts",
    ])
    const [author] = authors
    // Whole on the page, wrapped where it must be: nothing repeats it in a title.
    expect(author?.hasAttribute("title")).toBe(false)
    expect(author?.dataset.messageApp).toBe("charts/show")
    // Each name isolated from the words around it, and from the other.
    expect(
      [...(author?.querySelectorAll("bdi") ?? [])].map((name) => name.textContent),
    ).toEqual(["show", "charts"])
    const message = author?.closest<HTMLElement>(".workspace-message")
    expect(message?.dataset.role).toBe("user")
    // Plain text in reading order: the label, then the bubble it names.
    expect(author?.nextElementSibling?.classList.contains("workspace-bubble")).toBe(true)
    expect(message?.querySelector(".workspace-bubble")?.textContent).toBe("Plot May")
    // The person's own messages carry none.
    expect(host.querySelectorAll('.workspace-message[data-role="user"]').length).toBe(
      conversation.messages.filter((each) => each.role === "user").length + 1,
    )
  })

  it("E2-1 (#390): shows the bidi controls an app's names carry as U+FFFD, so none ends its isolation or turns its letters round", async () => {
    // The reviewer's strings: a stray PDI then an embedding; an override.
    const server = "a\u2069\u202Eb"
    const tool = "c\u2069\u2069\u202Bd"
    await shown(fakeSource(), {
      ...conversation,
      messages: [
        ...conversation.messages,
        {
          id: "m3",
          role: "user",
          at: 3,
          parts: [{ kind: "text", text: "Plot May" }],
          app: { server, tool },
        },
        {
          id: "m4",
          role: "user",
          at: 4,
          parts: [{ kind: "text", text: "Open it" }],
          app: { server: "evil\u202Egnp.exe", tool: "show" },
        },
      ],
      approval: {
        id: "app-ap",
        command: `${tool} {"text":"keep \u202E this"}`,
        reason: `An app asks to run ${tool} on evil\u202Egnp.exe`,
        origin: { kind: "app", server: "evil\u202Egnp.exe", tool },
        options: [
          { id: "allow", label: "Allow", choice: "once" },
          { id: "deny", label: "Deny", choice: "deny" },
        ],
        ask: "tool",
      },
    })
    const authors = [...host.querySelectorAll<HTMLElement>(".workspace-message-author")]
    expect(authors.map((author) => author.textContent)).toEqual([
      "Sent by c\uFFFD\uFFFD\uFFFDd, from a\uFFFD\uFFFDb",
      "Sent by show, from evil\uFFFDgnp.exe",
    ])
    expect(
      authors.map((author) =>
        [...author.querySelectorAll("bdi")].map((name) => name.textContent),
      ),
    ).toEqual([
      ["c\uFFFD\uFFFD\uFFFDd", "a\uFFFD\uFFFDb"],
      ["show", "evil\uFFFDgnp.exe"],
    ])
    // What the names are is kept as they came, for anything that reads it.
    expect(authors[0]?.dataset.messageApp).toBe(`${server}/${tool}`)
    const head = host.querySelector(".workspace-approval-head")
    expect(head?.textContent).toBe(
      "The evil\uFFFDgnp.exe app wants to run c\uFFFD\uFFFD\uFFFDd",
    )
    expect(
      [...(head?.querySelectorAll("bdi") ?? [])].map((name) => name.textContent),
    ).toEqual(["evil\uFFFDgnp.exe", "c\uFFFD\uFFFD\uFFFDd"])
    // In plain text, each name stays between its own FSI and PDI.
    expect(
      spoken(
        approvalHead({ origin: { kind: "app", server, tool }, ask: "tool" }, "Claude"),
      ),
    ).toBe(
      "The \u2068a\uFFFD\uFFFDb\u2069 app wants to run \u2068c\uFFFD\uFFFD\uFFFDd\u2069",
    )
    // The gateway's title repeats both names; the card isolates them, and
    // the message's own words keep the control they were sent with.
    const reason = host.querySelector(".workspace-approval-reason")
    expect(reason?.textContent).toBe(
      "An app asks to run c\uFFFD\uFFFD\uFFFDd on evil\uFFFDgnp.exe",
    )
    expect(
      [...(reason?.querySelectorAll("bdi") ?? [])].map((name) => name.textContent),
    ).toEqual(["c\uFFFD\uFFFD\uFFFDd", "evil\uFFFDgnp.exe"])
    expect(
      spoken(
        approvalReason({
          origin: { kind: "app", server: "evil\u202Egnp.exe", tool },
          reason: `An app asks to run ${tool} on evil\u202Egnp.exe`,
        }),
      ),
    ).toBe(
      "An app asks to run \u2068c\uFFFD\uFFFD\uFFFDd\u2069 on \u2068evil\uFFFDgnp.exe\u2069",
    )
    const command = host.querySelector(".workspace-approval-command")
    expect(command?.querySelector("bdi")?.textContent).toBe("c\uFFFD\uFFFD\uFFFDd")
    expect(command?.textContent).toContain('{"text":"keep \u202E this"}')
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
              scrollRef={createRef()}
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
    // The first snapshot and what was already shown stay put.
    expect(rising).not.toContain("one")
    expect(rising).not.toContain("two")
    // Later additions rise; the initial snapshot is held still.
    expect(rising).toHaveLength(0)
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

  it("sends a refused message again, or lets it go, from the composer recovery row", async () => {
    const source = fakeSource()
    source.refuse("send", "unavailable")
    const { store } = await shown(source)
    await act(async () => {
      await store.dispatch(
        sendMessage({ initiator: "person", sessionId: "b", text: "one" }),
      )
    })
    const unsent = (label: string) =>
      [...host.querySelectorAll<HTMLButtonElement>("[data-send-failure] button")].find(
        (button) => button.textContent === label,
      )
    await act(async () => {
      unsent("Send Again")?.click()
      await settle()
    })
    expect(source.calls.filter((call) => call[0] === "send")).toHaveLength(2)
    expect(host.querySelectorAll("[data-send-failure]")).toHaveLength(1)
    await act(async () => {
      unsent("Discard")?.click()
    })
    expect(host.querySelector("[data-send-failure]")).toBeNull()
    expect(store.getState().workspace.outbox.b).toBeUndefined()
  })

  it("shows the deadline that will answer a review nobody has answered", async () => {
    const approval = conversation.approval
    if (!approval) throw new Error("the sample asks for nothing")
    await shown(fakeSource(), {
      ...conversation,
      approval: {
        ...approval,
        deadline: {
          waitMs: 30_000,
          dueAtMs: 31_000,
          effect: "deny",
          reason:
            "No human answered in time. Continue without this tool or try another approach.",
          stopsTurn: false,
        },
      },
    })
    const deadline = host.querySelector<HTMLElement>("[data-deadline]")
    expect(deadline?.dataset.deadline).toBe("deny")
    expect(deadline?.textContent).toBe(
      "If nobody answers within 30 seconds, this is denied automatically.",
    )
    expect(host.querySelector(".workspace-approval-reason")?.textContent).toBe(
      "Runs the tests.",
    )
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
      ["approve", "b", "ap", "once", "person", "once"],
    ])
  })

  it("offers no answer on an approval whose review is still being read, and offers reading it again when it could not be", async () => {
    const source = fakeSource()
    const approval = conversation.approval
    if (!approval) throw new Error("the sample asks for nothing")
    await shown(source, {
      ...conversation,
      approval: { ...approval, loading: "reading" },
    })
    expect(host.querySelector(".workspace-approval-loading")?.textContent).toBe(
      "Loading this review before you can answer…",
    )
    expect(buttons().every(([, disabled]) => disabled)).toBe(true)
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".workspace-approval [data-primary]")?.click()
    })
    expect(source.calls.filter((call) => call[0] === "approve")).toEqual([])
    await shown(source, { ...conversation, approval: { ...approval, loading: "failed" } })
    expect(host.querySelector(".workspace-approval-loading")?.textContent).toContain(
      "This review could not be loaded.",
    )
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".workspace-approval-retry")?.click()
    })
    expect(source.calls.filter((call) => call[0] === "retryReview")).toEqual([
      ["retryReview", conversation.sessionId, approval.id],
    ])
  })

  it("answers the clicked option when two allow the same way", async () => {
    const source = fakeSource()
    const approval = conversation.approval
    if (!approval) throw new Error("the sample asks for nothing")
    await shown(source, {
      ...conversation,
      approval: {
        ...approval,
        options: [
          { id: "ship", label: "Ship it", choice: "once" },
          { id: "run", label: "Run it", choice: "once" },
        ],
      },
    })
    const run = [...host.querySelectorAll("button")].find(
      (button) => button.textContent === "Run it",
    )
    await act(async () => {
      run?.click()
    })
    expect(source.calls.filter((call) => call[0] === "approve")).toEqual([
      ["approve", "b", "ap", "once", "person", "run"],
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
        options: [
          { id: "allow", label: "Allow", choice: "once" },
          { id: "deny", label: "Deny", choice: "deny" },
        ],
        ask: "tool",
      },
    })
    expect(head()).toBe("The mcptest app wants to run app_delete_row")
    expect(head()).not.toContain(agent)
    // The names an app chose are isolated from the words around them (E1-8).
    expect(
      [...(host.querySelectorAll(".workspace-approval-head bdi") ?? [])].map(
        (name) => name.textContent,
      ),
    ).toEqual(["mcptest", "app_delete_row"])
    expect(card()?.dataset.origin).toBe("app")
  })

  it("offers only the answers its review carries (#444)", async () => {
    const source = fakeSource()
    await shown(source, {
      ...conversation,
      approval: {
        id: "app-ap",
        command: "app_delete_row {}",
        reason: "An app asks to run app_delete_row on mcptest",
        origin: { kind: "app", server: "mcptest", tool: "app_delete_row" },
        options: [
          { id: "allow", label: "Allow", choice: "once" },
          { id: "deny", label: "Deny", choice: "deny" },
        ],
        ask: "tool",
      },
    })
    // Deny stays at the left; the review's allow is the primary button, in the review's words.
    expect(buttons()).toEqual([
      ["Deny", false],
      ["Allow", false],
    ])
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".workspace-approval [data-primary]")?.click()
    })
    expect(
      source.calls.filter((call) => call[0] === "approve" || call[0] === "deny"),
    ).toEqual([["approve", "b", "app-ap", "once", "person", "allow"]])
  })

  it("D19 (#390): says what is asked — an app's message is not a tool to run — over every combination the client lets through", async () => {
    const app = { kind: "app", server: "mcptest", tool: "show_rows" } as const
    expect(approvalHead({ origin: app, ask: "message" }, "Claude")).toEqual([
      "The ",
      { name: "mcptest" },
      " app",
      " wants to send a message as you",
    ])
    expect(spoken(approvalHead({ origin: app, ask: "tool" }, "Claude"))).toBe(
      "The \u2068mcptest\u2069 app wants to run \u2068show_rows\u2069",
    )
    expect(
      spoken(approvalHead({ origin: { kind: "agent" }, ask: "tool" }, "Claude")),
    ).toBe("Claude wants to run a command")
    await shown(fakeSource(), {
      ...conversation,
      approval: {
        id: "app-message",
        command: 'show_rows {"text":"Plot May next to April"}',
        reason: "The show_rows app on mcptest asks to send a message as you",
        origin: app,
        options: [
          { id: "allow", label: "Allow", choice: "once" },
          { id: "deny", label: "Deny", choice: "deny" },
        ],
        ask: "message",
      },
    })
    const card = host.querySelector<HTMLElement>(".workspace-approval")
    expect(card?.querySelector(".workspace-approval-head")?.textContent).toBe(
      "The mcptest app wants to send a message as you",
    )
    expect(card?.querySelector(".workspace-approval-head bdi")?.textContent).toBe(
      "mcptest",
    )
    expect(card?.dataset.origin).toBe("app")
    expect(card?.dataset.ask).toBe("message")
    // What is sent is shown whole: the app's tool and the message's words.
    expect(card?.querySelector(".workspace-approval-command")?.textContent).toContain(
      '{"text":"Plot May next to April"}',
    )
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

const claudeRuntime = {
  agent: "claude",
  provider: "anthropic",
  model: "claude-opus-5",
  modelName: "Claude",
  workspace: "/tmp",
  contextWindowTokens: 200000,
  reasoning: true,
}

describe("provider recovery ownership", () => {
  it("keeps the auth terminal when a local retry hides recovery", async () => {
    const source = fakeSource()
    source.hold("send")
    const transcript = transcriptFrom(
      view("b", {
        runtime: claudeRuntime,
        messages: [
          {
            executionId: "m1",
            userText: "Hello",
            attachments: [],
            files: [],
            status: "failed",
            authenticationRequired: true,
            error: "Internal error: OAuth session expired",
            parts: [],
          },
        ],
      }),
      1,
      () => 1,
      GATEWAY_NOW_MS,
    )
    const { store } = await shown(source, transcript)
    expect(host.querySelector(".provider-sign-in")).not.toBeNull()
    expect(host.querySelector("[data-turn-terminal]")).toBeNull()
    expect(host.textContent).not.toContain("OAuth session expired")
    let sent: Promise<unknown> | undefined
    await act(async () => {
      sent = store.dispatch(
        sendMessage({ initiator: "person", sessionId: "b", text: "Try again" }),
      )
      await settle()
    })
    expect(host.querySelector(".provider-sign-in")).toBeNull()
    expect(host.querySelector("[data-turn-terminal]")?.textContent).toContain("Failed")
    expect(
      host.querySelector("[data-turn-terminal]")?.getAttribute("data-turn-error"),
    ).toBe("Internal error: OAuth session expired")
    source.refuse("send", "unavailable")
    await act(async () => {
      await source.release("send")
      await sent
    })
    expect(host.querySelector(".provider-sign-in")).toBeNull()
    expect(
      host.querySelector("[data-turn-terminal]")?.getAttribute("data-turn-error"),
    ).toBe("Internal error: OAuth session expired")
  })

  it("keeps an independent required-work failure beside the sign-in card", async () => {
    const transcript = transcriptFrom(
      view("b", {
        runtime: claudeRuntime,
        messages: [
          {
            executionId: "m1",
            userText: "Hello",
            attachments: [],
            files: [],
            status: "failed",
            authenticationRequired: true,
            requiredWorkFailed: true,
            error: "The turn could not complete all required work.",
            parts: [],
          },
        ],
      }),
      1,
      () => 1,
      GATEWAY_NOW_MS,
    )
    await shown(fakeSource(), transcript)
    expect(host.querySelector(".provider-sign-in")).not.toBeNull()
    expect(
      host.querySelector("[data-turn-terminal]")?.getAttribute("data-turn-error"),
    ).toBe("The turn could not complete all required work.")
  })

  it("retires recovery while a newer user-only message is sending or failed", async () => {
    const source = fakeSource()
    source.hold("send")
    const { store } = await shown(source, {
      ...emptyTranscript("b"),
      revision: 1,
      authenticationRefusal: "m1",
      latestInputId: "m1",
      agent: "codex",
      messages: [conversation.messages[0]],
    })
    expect(host.querySelector(".provider-sign-in")).not.toBeNull()
    expect(host.querySelector(".provider-sign-in button")?.textContent).toContain("Codex")
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
    expect(host.querySelector("[data-send-failure]")).not.toBeNull()
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
                GATEWAY_NOW_MS,
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

it.each([undefined, "unknown-provider"])(
  "does not infer login from the model when runtime agent is %s",
  async (agent) => {
    await shown(fakeSource(), {
      ...emptyTranscript("b"),
      revision: 1,
      authenticationRefusal: "m1",
      latestInputId: "m1",
      agent,
      messages: [conversation.messages[0]],
    })
    expect(host.querySelector(".provider-sign-in")).toBeNull()
  },
)

it("keeps an older failed outbox message from hiding a later observed refusal", async () => {
  const source = fakeSource()
  source.hold("send")
  const { store } = await shown(source, { ...emptyTranscript("b"), revision: 1 })
  store.dispatch(followWorkspace())
  let sent: Promise<unknown> | undefined
  await act(async () => {
    sent = store.dispatch(
      sendMessage({ initiator: "person", sessionId: "b", text: "Older local failure" }),
    )
    await settle()
  })
  source.refuse("send", "unavailable")
  await act(async () => {
    await source.release("send")
    await sent
  })
  const [older] = store.getState().workspace.outbox.b
  expect(older.observedInput).toBeNull()
  const refusal = (executionId: string, revision: number) =>
    transcriptFrom(
      view("b", {
        runtime: {
          agent: "codex",
          provider: "openai",
          model: "unknown",
          modelName: "Unknown",
          workspace: "/tmp",
          contextWindowTokens: 200000,
          reasoning: true,
        },
        messages: [
          {
            executionId,
            userText: "Later wire input",
            attachments: [],
            files: [],
            parts: [],
            status: "failed",
            authenticationRequired: true,
          },
        ],
      }),
      revision,
      () => 1,
      GATEWAY_NOW_MS,
    )
  for (const revision of [2, 3]) {
    await act(async () => {
      const transcript = refusal("B", revision)
      source.transcripts.set("b", transcript)
      source.emit({ kind: "transcript", transcript })
      await settle()
    })
    expect(host.querySelector(".provider-sign-in")).not.toBeNull()
  }
  expect(host.textContent).toContain("Older local failure")
  source.hold("send")
  await act(async () => {
    sent = store.dispatch(
      sendMessage({ initiator: "person", sessionId: "b", text: "Newer local input" }),
    )
    await settle()
  })
  expect(host.querySelector(".provider-sign-in")).toBeNull()
  await act(async () => {
    const transcript = refusal("B", 4)
    source.transcripts.set("b", transcript)
    source.emit({ kind: "transcript", transcript })
    await settle()
  })
  expect(host.querySelector(".provider-sign-in")).toBeNull()
  await act(async () => {
    await source.release("send")
    await sent
  })
  expect(host.querySelector(".provider-sign-in")).toBeNull()
  expect(store.getState().workspace.outbox.b).toHaveLength(2)
  await act(async () => {
    const transcript = refusal("D", 5)
    source.transcripts.set("b", transcript)
    source.emit({ kind: "transcript", transcript })
    await settle()
  })
  expect(host.querySelector(".provider-sign-in")).not.toBeNull()
  source.hold("send")
  await act(async () => {
    sent = store.dispatch(
      resendMessage({ sessionId: "b", messageId: older.id, initiator: "person" }),
    )
    await settle()
  })
  expect(store.getState().workspace.outbox.b[0].observedInput).toBe("D")
  expect(host.querySelector(".provider-sign-in")).toBeNull()
  await act(async () => {
    const transcript = refusal("D", 6)
    source.transcripts.set("b", transcript)
    source.emit({ kind: "transcript", transcript })
    await source.release("send")
    await sent
  })
  expect(host.querySelector(".provider-sign-in")).toBeNull()
})

it("observes heading visibility without forcing the mounting layout", async () => {
  let visibility: IntersectionObserverCallback | null = null
  const observed = vi.fn()
  const disconnected = vi.fn()
  class HeadingObserver {
    constructor(callback: IntersectionObserverCallback) {
      visibility = callback
    }
    observe = observed
    disconnect = disconnected
  }
  const previous = globalThis.IntersectionObserver
  Object.assign(globalThis, { IntersectionObserver: HeadingObserver })
  const geometry = vi
    .spyOn(Element.prototype, "getBoundingClientRect")
    .mockImplementation(() => {
      throw new Error("forced mounting layout")
    })
  const reported = vi.fn()
  try {
    await shown(fakeSource(), conversation, reported)
    expect(reported).not.toHaveBeenCalled()
    expect(observed).toHaveBeenCalledWith(host.querySelector(".workspace-heading h2"))
    const notify = visibility as IntersectionObserverCallback | null
    expect(notify).not.toBeNull()
    await act(async () =>
      notify?.(
        [{ isIntersecting: true } as IntersectionObserverEntry],
        {} as IntersectionObserver,
      ),
    )
    await act(async () =>
      notify?.(
        [{ isIntersecting: false } as IntersectionObserverEntry],
        {} as IntersectionObserver,
      ),
    )
    expect(reported.mock.calls).toEqual([[true], [false]])
    await act(async () => root.unmount())
    expect(disconnected).toHaveBeenCalledTimes(2)
  } finally {
    geometry.mockRestore()
    Object.assign(globalThis, { IntersectionObserver: previous })
  }
})

it("pins after observed layout, respects scrolling away, and rejects late deliveries", async () => {
  let notify: ResizeObserverCallback | null = null
  class LayoutObserver {
    constructor(callback: ResizeObserverCallback) {
      notify = callback
    }
    observe() {}
    disconnect() {}
  }
  const previous = globalThis.ResizeObserver
  Object.assign(globalThis, { ResizeObserver: LayoutObserver })
  let laidOut = false
  let height = 500
  const geometry = vi
    .spyOn(HTMLElement.prototype, "scrollHeight", "get")
    .mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains("workspace-transcript")) {
        expect(laidOut, "scroll read before observed layout").toBe(true)
        return height
      }
      return 0
    })
  let removed = false
  try {
    await shown()
    expect(geometry).not.toHaveBeenCalled()
    const scroller = host.querySelector<HTMLElement>(".workspace-transcript")
    expect(scroller).not.toBeNull()
    if (!scroller) return
    Object.defineProperty(scroller, "clientHeight", { configurable: true, value: 100 })
    const deliver = notify as ResizeObserverCallback | null
    expect(deliver).not.toBeNull()
    laidOut = true
    await act(async () => deliver?.([], {} as ResizeObserver))
    expect(scroller.scrollTop).toBe(500)
    scroller.scrollTop = 0
    scroller.dispatchEvent(new Event("scroll"))
    height = 700
    await act(async () => deliver?.([], {} as ResizeObserver))
    expect(scroller.scrollTop).toBe(0)
    scroller.scrollTop = 600
    scroller.dispatchEvent(new Event("scroll"))
    height = 900
    await act(async () => deliver?.([], {} as ResizeObserver))
    expect(scroller.scrollTop).toBe(900)
    await act(async () => root.unmount())
    removed = true
    scroller.scrollTop = 10
    await act(async () => deliver?.([], {} as ResizeObserver))
    expect(scroller.scrollTop).toBe(10)
  } finally {
    if (!removed) await act(async () => root.unmount())
    geometry.mockRestore()
    Object.assign(globalThis, { ResizeObserver: previous })
  }
})

it("offers the way back to the latest only half a screen from the end, and returns there within the motion budget", async () => {
  let notify: ResizeObserverCallback | null = null
  class LayoutObserver {
    constructor(callback: ResizeObserverCallback) {
      notify = callback
    }
    observe() {}
    disconnect() {}
  }
  const previous = globalThis.ResizeObserver
  Object.assign(globalThis, { ResizeObserver: LayoutObserver })
  const geometry = vi
    .spyOn(HTMLElement.prototype, "scrollHeight", "get")
    .mockImplementation(function (this: HTMLElement) {
      return this.classList.contains("workspace-transcript") ? 1000 : 0
    })
  // Frames the test turns by hand, so nothing is left running after it.
  const frames = new Map<number, FrameRequestCallback>()
  let nextFrame = 0
  const request = vi
    .spyOn(window, "requestAnimationFrame")
    .mockImplementation((callback) => {
      frames.set(++nextFrame, callback)
      return nextFrame
    })
  const cancelFrame = vi
    .spyOn(window, "cancelAnimationFrame")
    .mockImplementation((id) => void frames.delete(id))
  const clock = vi.spyOn(performance, "now").mockReturnValue(0)
  const turn = async (at: number) => {
    const due = [...frames.values()]
    frames.clear()
    await act(async () => due.forEach((callback) => callback(at)))
  }
  try {
    await shown()
    const scroller = host.querySelector<HTMLElement>(".workspace-transcript")
    const jump = host.querySelector<HTMLButtonElement>(".workspace-jump")
    if (!scroller || !jump) throw new Error("no transcript")
    Object.defineProperty(scroller, "clientHeight", { configurable: true, value: 200 })
    await act(async () =>
      (notify as ResizeObserverCallback | null)?.([], {} as ResizeObserver),
    )
    expect(scroller.scrollTop).toBe(1000)
    expect(jump.hasAttribute("data-hidden")).toBe(true)
    expect(jump.tabIndex).toBe(-1)

    // A nudge up is not away.
    scroller.scrollTop = 750
    await act(async () => scroller.dispatchEvent(new Event("scroll")))
    expect(jump.hasAttribute("data-hidden")).toBe(true)

    scroller.scrollTop = 200
    await act(async () => scroller.dispatchEvent(new Event("scroll")))
    expect(jump.hasAttribute("data-hidden")).toBe(false)
    expect(jump.getAttribute("aria-label")).toBe("Jump to Latest")

    // The stylesheet's duration (`--desktop-slow`), which reduced motion makes 0.
    scroller.style.setProperty("--desktop-slow", "240ms")
    await act(async () => jump.click())
    await turn(120)
    expect(scroller.scrollTop).toBeGreaterThan(200)
    expect(scroller.scrollTop).toBeLessThan(800)
    await turn(240)
    expect(scroller.scrollTop).toBe(800)
    expect(frames.size).toBe(0)
    await act(async () => scroller.dispatchEvent(new Event("scroll")))
    expect(jump.hasAttribute("data-hidden")).toBe(true)

    // The reader taking the scroll back mid-return keeps where they are.
    scroller.scrollTop = 100
    await act(async () => scroller.dispatchEvent(new Event("scroll")))
    await act(async () => jump.click())
    await turn(60)
    const reached = scroller.scrollTop
    scroller.dispatchEvent(new Event("wheel"))
    expect(frames.size).toBe(0)
    expect(scroller.scrollTop).toBe(reached)

    // Reduced motion: straight there, no frames.
    scroller.style.setProperty("--desktop-slow", "0ms")
    await act(async () => jump.click())
    expect(scroller.scrollTop).toBe(1000)
    expect(frames.size).toBe(0)
  } finally {
    await act(async () => root.unmount())
    root = createRoot(host)
    geometry.mockRestore()
    request.mockRestore()
    cancelFrame.mockRestore()
    clock.mockRestore()
    Object.assign(globalThis, { ResizeObserver: previous })
  }
})
