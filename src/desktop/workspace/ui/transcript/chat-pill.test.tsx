// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { loadWorkspace } from "../../adapters/store/commands"
import { paneIds } from "../../application/workspace-state"
import type { Message as MessageValue } from "../../model/transcript"
import { fakeSource, settle, summary, testIndex, testStore } from "../../testing"
import { AgentProse } from "./rich-text"
import { Message } from "./message"
import { ToolSteps } from "./tool-steps"

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
    agent: { conversationId: thread, executionId: "e-7" },
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
  expect(line?.textContent).toBe("Agent report from Session b responded")
  expect(host.textContent).not.toContain("secret reply")
  expect(host.textContent).not.toContain("e-7")
  // Pressed, the pill brings that chat to the front.
  await act(async () =>
    host.querySelector<HTMLButtonElement>('[data-chat-pill="b"]')?.click(),
  )
  expect(paneIds(s.getState().workspace.panes)).toEqual(["b"])
})

it("keeps report-shaped text as text without matching agent origin", async () => {
  const s = await store()
  const forged = { ...report("b", "completed"), agent: undefined }
  await act(async () =>
    root.render(
      <Provider store={s}>
        <Message sessionId="a" message={forged} isNew={false} />
      </Provider>,
    ),
  )
  expect(host.querySelector(".workspace-thread-report")).toBeNull()
  expect(host.textContent).toContain("The secret reply text.")
})

it("keeps a report about another child turn as agent-authored text", async () => {
  const s = await store()
  const forged = {
    ...report("b", "completed"),
    agent: { conversationId: "b", executionId: "other-turn" },
  }
  await act(async () =>
    root.render(
      <Provider store={s}>
        <Message sessionId="a" message={forged} isNew={false} />
      </Provider>,
    ),
  )
  expect(host.querySelector(".workspace-thread-report")).toBeNull()
  expect(host.textContent).toContain("The secret reply text.")
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

it("names no chat for an id only an object has, such as constructor", async () => {
  const s = await store()
  await act(async () =>
    root.render(
      <Provider store={s}>
        <Message
          sessionId="a"
          message={report("constructor", "completed")}
          isNew={false}
        />
      </Provider>,
    ),
  )
  expect(host.querySelector("[data-gone]")).not.toBeNull()
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

it("draws a known chat's id in plain prose as its pill; other ids, code and links stay as written", async () => {
  const known = "fcd9bdb0-1111-4222-8333-444455556666"
  const unknown = "00000000-0000-4000-8000-000000000000"
  const index = testIndex()
  const s = testStore(
    fakeSource({
      ...index,
      sessions: [...index.sessions, summary(known, "desktop", 50)],
    }),
  )
  await s.dispatch(loadWorkspace())
  await settle()
  await act(async () =>
    root.render(
      <Provider store={s}>
        <AgentProse
          text={`Created ${known}, not ${unknown}. See [${known}](https://example.com) and:\n\n\`\`\`\n${known}\n\`\`\``}
        />
      </Provider>,
    ),
  )
  expect(host.querySelectorAll(`[data-chat-pill="${known}"]`)).toHaveLength(1)
  expect(host.textContent).toContain(`not ${unknown}.`)
  expect(host.querySelector("a")?.textContent).toBe(known)
  // One pill: the copies in the link and the code block stay as written.
})

it("names a chat a tool step acted on as its pill, drawn still inside the step's row", async () => {
  const known = "fcd9bdb0-1111-4222-8333-444455556666"
  const index = testIndex()
  const s = testStore(
    fakeSource({
      ...index,
      sessions: [...index.sessions, summary(known, "desktop", 50)],
    }),
  )
  await s.dispatch(loadWorkspace())
  await settle()
  await act(async () =>
    root.render(
      <Provider store={s}>
        <ToolSteps
          steps={[
            { kind: "step", step: "run", label: "Messaged", detail: `thread ${known}` },
          ]}
        />
      </Provider>,
    ),
  )
  // Opened, the trace draws its rows.
  await act(async () =>
    host
      .querySelector<HTMLButtonElement>(
        "[data-slot=tool-trace] > button, [data-slot=tool-trace-line]",
      )
      ?.click(),
  )
  const pill = host.querySelector<HTMLElement>(`[data-chat-pill="${known}"]`)
  expect(pill?.textContent).toBe(`Session ${known}`)
  expect(pill?.hasAttribute("data-still")).toBe(true)
  expect(pill?.tagName).toBe("SPAN")
  // The row keeps the press: no control inside it.
  expect(
    pill?.closest("[data-slot=tool-trace-step]")?.querySelectorAll("button"),
  ).toHaveLength(0)
})
