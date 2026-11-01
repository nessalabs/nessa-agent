// @vitest-environment jsdom
/**
 * A run of steps in a message is over once its turn is, or once the agent has
 * written past it with none of its calls still at work.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { Message as MessageValue, StepStatus } from "../../model/transcript"
import { testStore } from "../../testing"
import { Message } from "./message"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
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

const reply = (first: StepStatus, working: boolean): MessageValue => ({
  id: "m",
  role: "agent",
  at: 1,
  ...(working ? { working: true as const } : {}),
  parts: [
    {
      kind: "step",
      step: "run",
      label: "Run",
      detail: "pnpm dev",
      id: "t1",
      status: first,
    },
    { kind: "text", text: "The server is up; reading the logs next." },
    {
      kind: "step",
      step: "read",
      label: "Read",
      detail: "dev.log",
      id: "t2",
      status: "running",
    },
  ],
})

/** Whether each run of the message is drawn as running. */
async function runs(message: MessageValue) {
  await act(async () =>
    root.render(
      <Provider store={testStore()}>
        <Message sessionId="a" message={message} isNew={false} />
      </Provider>,
    ),
  )
  return [...host.querySelectorAll("[data-slot=tool-trace]")].map(
    (trace) => trace.getAttribute("data-status") === "running",
  )
}

it("ends a run the agent wrote past once its calls have settled", async () => {
  expect(await runs(reply("completed", true))).toEqual([false, true])
})

it("keeps a run the agent wrote past at work while a call in it still runs", async () => {
  expect(await runs(reply("running", true))).toEqual([true, true])
})

it("ends every run once the turn has", async () => {
  expect(await runs(reply("stopped", false))).toEqual([false, false])
})

it("draws no run in the compact view, only what the agent wrote", async () => {
  await act(async () =>
    root.render(
      <Provider store={testStore()}>
        <Message
          sessionId="a"
          message={reply("completed", true)}
          isNew={false}
          view="compact"
        />
      </Provider>,
    ),
  )
  expect(host.querySelectorAll("[data-slot=tool-trace]")).toHaveLength(0)
  expect(host.textContent).toContain("The server is up; reading the logs next.")
})

it("draws nothing in the compact view for a reply that was only tool runs", async () => {
  const only: MessageValue = {
    id: "m",
    role: "agent",
    at: 1,
    parts: [{ kind: "step", step: "run", label: "Run", id: "t1", status: "completed" }],
  }
  await act(async () =>
    root.render(
      <Provider store={testStore()}>
        <Message sessionId="a" message={only} isNew={false} view="compact" />
      </Provider>,
    ),
  )
  expect(host.querySelector(".workspace-message")).toBeNull()
})
