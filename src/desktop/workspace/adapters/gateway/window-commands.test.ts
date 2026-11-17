import type { WorkspaceAnswerParams, WorkspaceCommand } from "@nessa/client"
import { describe, expect, it } from "vitest"
import { settle } from "../../testing"
import type { WindowChange, WindowChangeOutcome } from "../../model/window-change"
import {
  clientsSeen,
  windowChange,
  windowCommandInbox,
  type WindowCommandClient,
} from "./window-commands"

const WINDOW = "7d9f2a4e-1b3c-4d5e-8f60-718293a4b5c6"
const OTHER = "1c4c9f1f-7054-4b62-8e66-1f8f5f8f9b22"
const SESSION = "0b3b8f0e-6f43-4a51-9d55-0e7e4f7f8a11"

function inbox(outcome: (change: WindowChange) => Promise<WindowChangeOutcome>) {
  let send: ((command: WorkspaceCommand) => void) | undefined
  const answers: WorkspaceAnswerParams[] = []
  const carried: WindowChange[] = []
  const client: WindowCommandClient = {
    workspace: {
      answer: async (answer) => {
        answers.push(answer)
        return { settled: true }
      },
    },
    on: (_event, handler) => {
      send = handler
      return () => (send = undefined)
    },
  }
  const stop = windowCommandInbox({
    clients: (listener) => {
      listener(client)
      return () => {}
    },
    publisher: { windowId: WINDOW, publish: () => {}, settled: async () => 9 },
  }).listen((change) => {
    carried.push(change)
    return outcome(change)
  })
  return {
    answers,
    carried,
    stop,
    send: async (command: WorkspaceCommand) => {
      await settle()
      send?.(command)
      await settle()
    },
  }
}

describe("the changes asked of this window", () => {
  it("are carried out in order and answered with the revision that shows them", async () => {
    const window = inbox(async (change) =>
      change.kind === "close"
        ? { kind: "refused", code: "not_on_screen" }
        : { kind: "applied" },
    )
    await window.send({
      windowId: WINDOW,
      commandId: 1,
      action: { kind: "panel", open: true },
    })
    await window.send({
      windowId: WINDOW,
      commandId: 2,
      action: { kind: "close_pane", conversationId: SESSION },
    })
    expect(window.carried).toEqual([
      { kind: "panel", open: true },
      { kind: "close", sessionId: SESSION },
    ])
    expect(window.answers).toEqual([
      { windowId: WINDOW, commandId: 1, outcome: "applied", revision: 9 },
      { windowId: WINDOW, commandId: 2, outcome: "refused", code: "not_on_screen" },
    ])
  })

  it("leaves another window's change, or one it cannot read, unanswered", async () => {
    const window = inbox(async () => ({ kind: "applied" }))
    await window.send({
      windowId: OTHER,
      commandId: 1,
      action: { kind: "sidebar", open: true },
    })
    await window.send({ windowId: WINDOW, commandId: 2, action: { kind: "sidebar" } })
    expect(window.carried).toEqual([])
    expect(window.answers).toEqual([])
  })

  it("reads each kind with exactly what it needs", () => {
    expect(windowChange({ kind: "session_list", open: false })).toEqual({
      kind: "sessionList",
      open: false,
    })
    expect(windowChange({ kind: "open_conversation", conversationId: SESSION })).toEqual({
      kind: "open",
      sessionId: SESSION,
      place: "beside",
    })
    expect(windowChange({ kind: "focus_pane", conversationId: "sample-a" })).toBeNull()
    expect(windowChange({ kind: "show" })).toBeNull()
  })
})

describe("the clients the window publishes on", () => {
  function client(name: string) {
    const handlers: Array<(command: WorkspaceCommand) => void> = []
    const answers: WorkspaceAnswerParams[] = []
    const value: WindowCommandClient & { name: string } = {
      name,
      workspace: {
        answer: async (answer) => {
          answers.push(answer)
          return { settled: true }
        },
      },
      on: (_event, handler) => {
        handlers.push(handler)
        return () => handlers.splice(handlers.indexOf(handler), 1)
      },
    }
    return { value, handlers, answers }
  }

  it("are listened on as each comes, the one before let go, and not before the first connects", async () => {
    const first = client("first")
    const second = client("second")
    let next: Promise<WindowCommandClient> = Promise.reject(new Error("unavailable"))
    const seen = clientsSeen(() => next)
    windowCommandInbox({
      clients: seen.follow,
      publisher: { windowId: WINDOW, publish: () => {}, settled: async () => 4 },
    }).listen(async () => ({ kind: "applied" }))
    await expect(seen.connected()).rejects.toThrow("unavailable")
    next = Promise.resolve(first.value)
    await seen.connected()
    await seen.connected()
    expect(first.handlers).toHaveLength(1)
    // The connection closed for good: the window publishes on a new client.
    next = Promise.resolve(second.value)
    await seen.connected()
    expect(first.handlers).toHaveLength(0)
    expect(second.handlers).toHaveLength(1)
    second.handlers[0]({
      windowId: WINDOW,
      commandId: 3,
      action: { kind: "sidebar", open: false },
    })
    await settle()
    expect(second.answers).toEqual([
      { windowId: WINDOW, commandId: 3, outcome: "applied", revision: 4 },
    ])
  })
})
