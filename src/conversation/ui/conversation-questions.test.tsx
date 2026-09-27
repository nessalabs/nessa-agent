// @vitest-environment jsdom
/**
 * That an ask is answered under the agent's own field names, whatever they are,
 * and that Answer is offered only for an answer the agent will accept.
 *
 * Question keys come from the agent. Held in plain objects, `toString` and
 * `__proto__` read back what the object inherited rather than a selection, and
 * the first render threw. Required questions are the agent's other rule: an
 * answer that leaves one out is refused, so the panel must not offer to send it.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { act } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Conversation } from "../model"

const dispatch = vi.fn()
vi.mock("../adapters/store/hooks", () => ({ useConversationDispatch: () => dispatch }))
vi.mock("../adapters/store/slice", () => ({
  controlConversation: (request: unknown) => ({ type: "control", request }),
}))

const { ConversationQuestions } = await import("./conversation-questions")

// React only treats `act` as real in an environment that claims to support it.
;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

type Asked = NonNullable<Conversation["remote"]>["questions"][number]["questions"][number]

let host: HTMLDivElement
let root: Root

beforeEach(() => {
  dispatch.mockReset()
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
})

function question(key: string, required: boolean): Asked {
  return {
    key,
    prompt: `Asked under ${key}`,
    header: key,
    multiSelect: false,
    freeText: false,
    required,
    options: [
      { value: "a", label: "A" },
      { value: "b", label: "B" },
    ],
  }
}

function show(questions: Asked[]) {
  const conversation = {
    id: "conversation",
    controlPending: false,
    remote: {
      questions: [
        { executionId: "execution", questionId: "1", message: "Which?", questions },
      ],
    },
  } as unknown as Conversation
  act(() => {
    root.render(
      React.createElement(ConversationQuestions, {
        conversation,
        gatewayAvailable: true,
      }),
    )
  })
}

function choose(key: string, value: string) {
  const input = host.querySelector<HTMLInputElement>(
    `input[name="${CSS.escape(key)}"][value="${CSS.escape(value)}"]`,
  )
  if (!input) throw new Error(`no ${value} for ${key}`)
  act(() => input.click())
}

function write(key: string, words: string) {
  const input = host.querySelector<HTMLInputElement>(
    `input[name="${CSS.escape(`${key}_custom`)}"]`,
  )
  if (!input) throw new Error(`no own-words field for ${key}`)
  // React tracks the value it last rendered; setting it through the native
  // setter is what makes the input event read as a change.
  const setValue = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )?.set
  act(() => {
    setValue?.call(input, words)
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
}

function answer() {
  return [...host.querySelectorAll("button")].find(
    (button) => button.textContent === "Answer",
  ) as HTMLButtonElement
}

function sent() {
  return dispatch.mock.calls.map(([action]) => action.request.control.choices)
}

describe("an ask in the conversation", () => {
  it("answers questions whose keys an object would have inherited", () => {
    show([question("__proto__", false), question("toString", false)])
    expect(answer().disabled).toBe(true)
    choose("__proto__", "a")
    choose("toString", "b")
    act(() => answer().click())
    expect(sent()).toEqual([
      [
        { key: "__proto__", values: ["a"], ownWords: undefined },
        { key: "toString", values: ["b"], ownWords: undefined },
      ],
    ])
  })

  it("waits for every required question before offering to answer", () => {
    show([question("optional", false), question("needed", true)])
    choose("optional", "a")
    expect(answer().disabled).toBe(true)
    expect(host.querySelector('[aria-required="true"]')).not.toBeNull()
    choose("needed", "b")
    expect(answer().disabled).toBe(false)
  })

  it("does not take own words in place of a required choice", () => {
    // The agent required the choice field; prose travels in another, so
    // words alone would send it an answer its own schema refuses.
    show([{ ...question("needed", true), freeText: true }])
    write("needed", "somewhere else")
    expect(answer().disabled).toBe(true)
    choose("needed", "a")
    expect(answer().disabled).toBe(false)
  })

  it("takes own words alone for an optional question", () => {
    // The guard for the test above: the same words do reach the answer.
    show([{ ...question("optional", false), freeText: true }])
    write("optional", "somewhere else")
    expect(answer().disabled).toBe(false)
  })

  it("can still be skipped whole when a question is required", () => {
    show([question("needed", true)])
    const skip = [...host.querySelectorAll("button")].find(
      (button) => button.textContent === "Skip",
    ) as HTMLButtonElement
    act(() => skip.click())
    expect(sent()).toEqual([null])
  })
})
