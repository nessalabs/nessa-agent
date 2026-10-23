// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { loadWorkspace } from "../../adapters/store/commands"
import { paneIds } from "../../application/workspace-state"
import type { Message as MessageValue } from "../../model/transcript"
import { settle, testStore } from "../../testing"
import { AgentProse } from "./rich-text"
import { Message } from "./message"

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

async function store() {
  const s = testStore()
  await s.dispatch(loadWorkspace())
  await settle()
  return s
}

const report = (thread: string, outcome: string): MessageValue =>
  ({
    role: "user",
    id: "r1",
    parts: [
      {
        kind: "text",
        text: `[Thread report] Thread ${thread} finished the turn you sent it (execution e-7): ${outcome}.\n\nIts reply:\nThe secret reply text.`,
      },
    ],
  }) as unknown as MessageValue

it("shows a thread's report as one line: its chat and what it did, never the reply", async () => {
  const s = await store()
  await act(async () =>
    root.render(
      <Provider store={s}>
        <Message sessionId="a" message={report("b", "completed")} isNew={false} />
      </Provider>,
    ),
  )
  const line = host.querySelector(".workspace-thread-report")
  expect(line?.textContent).toBe("Session bresponded")
  expect(host.textContent).not.toContain("secret reply")
  expect(host.textContent).not.toContain("e-7")
  // Pressed, the pill brings that chat to the front.
  await act(async () =>
    host.querySelector<HTMLButtonElement>('[data-chat-pill="b"]')?.click(),
  )
  expect(paneIds(s.getState().workspace.panes)).toEqual(["b"])
})

it("says a gone chat is gone, and how a failed turn ended", async () => {
  const s = await store()
  await act(async () =>
    root.render(
      <Provider store={s}>
        <Message sessionId="a" message={report("nowhere", "failed")} isNew={false} />
      </Provider>,
    ),
  )
  expect(host.querySelector("[data-gone]")).not.toBeNull()
  expect(host.querySelector(".workspace-thread-report")?.textContent).toContain("failed")
})

it("draws a chat id an agent wrote as that chat's pill, and other code as code", async () => {
  const s = await store()
  await act(async () =>
    root.render(
      <Provider store={s}>
        <AgentProse text={"I created a new chat (ID `b`) and ran `ls -la`."} />
      </Provider>,
    ),
  )
  expect(host.querySelector('[data-chat-pill="b"]')?.textContent).toBe("Session b")
  expect([...host.querySelectorAll("code")].map((e) => e.textContent)).toEqual(["ls -la"])
})
