// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { followWorkspace, loadWorkspace } from "../../adapters/store/commands"
import { emptyTranscript, type Transcript } from "../../model/transcript"
import type { OwnerAsk as Ask } from "../../model/workspace-index"
import {
  fakeSource,
  settle,
  summary,
  testIndex,
  testStore,
  type FakeSource,
} from "../../testing"
import { OwnerAsk } from "./owner-ask"

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

const ask: Ask = {
  askId: "fedcba9876543210fedcba9876543210",
  question: "Allow full access in this chat?",
  choices: [
    { label: "Approve", consequence: "Raises this chat to Full access." },
    { label: "Not now", consequence: "Keeps this chat's access as it is." },
  ],
  recommended: 1,
  reason: "The toolchain installs outside the project.",
  access: {
    mode: "full",
    grants: [
      "Agents in this chat can run any command",
      "They won't ask you before they act",
      "You can undo this from Access, under the message box",
    ],
  },
}

/** The source says c's conversation anew, at its next revision: at work, or not. */
function sayConversation(
  source: FakeSource,
  working: boolean,
  access?: Transcript["access"],
): void {
  const held: Transcript = source.transcripts.get("c") ?? {
    ...emptyTranscript("c"),
    revision: 1,
  }
  const next: Transcript = {
    ...held,
    access: access ?? held.access ?? { mode: "ask", modes: ["ask", "auto", "full"] },
    activity: working ? { label: "Running the tests", since: 1 } : null,
    revision: held.revision + 1,
  }
  source.transcripts.set("c", next)
  // Drawn inside act, so the card's updates from it are all applied.
  act(() => source.emit({ kind: "transcript", transcript: next }))
}

const button = (name: string): HTMLButtonElement => {
  const found = [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (each) => each.textContent === name,
  )
  if (!found) throw new Error(`no button ${name}`)
  return found
}

it("sends Approve only once the session's conversation says it runs at the access", async () => {
  const index = testIndex()
  const source = fakeSource({
    ...index,
    sessions: index.sessions.map((session) =>
      session.id === "c" ? summary("c", "desktop", 100, "needs-you", { ask }) : session,
    ),
  })
  // The gateway takes the change, but its conversation has not said so yet.
  source.setAccess = (...args) => {
    source.calls.push(["setAccess", ...args])
    return Promise.resolve()
  }
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  store.dispatch(followWorkspace())
  sayConversation(source, false)
  await settle()
  await act(async () =>
    root.render(
      <Provider store={store}>
        <OwnerAsk sessionId="c" />
      </Provider>,
    ),
  )
  await act(async () => button("Approve").click())
  await act(async () => settle(10))
  const sends = () => source.calls.filter((call) => call[0] === "send")
  expect(source.calls.filter((call) => call[0] === "setAccess")).toHaveLength(1)
  expect(sends()).toEqual([])
  expect(host.querySelector("[aria-busy='true']")).not.toBeNull()
  // A change still on its way is not in effect either.
  sayConversation(source, false, {
    mode: "ask",
    modes: ["ask", "auto", "full"],
    changing: "full",
  })
  await act(async () => settle(10))
  expect(sends()).toEqual([])
  sayConversation(source, false, { mode: "full", modes: ["ask", "auto", "full"] })
  await act(async () => settle(10))
  expect(sends()).toHaveLength(1)
  expect(sends()[0]?.[1]).toMatchObject({ text: "Approve" })
})

it("sends no Approve when the ask leaves before the access arrives", async () => {
  const index = testIndex()
  const source = fakeSource({
    ...index,
    sessions: index.sessions.map((session) =>
      session.id === "c" ? summary("c", "desktop", 100, "needs-you", { ask }) : session,
    ),
  })
  source.setAccess = (...args) => {
    source.calls.push(["setAccess", ...args])
    return Promise.resolve()
  }
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  store.dispatch(followWorkspace())
  sayConversation(source, false)
  await settle()
  await act(async () =>
    root.render(
      <Provider store={store}>
        <OwnerAsk sessionId="c" />
      </Provider>,
    ),
  )
  await act(async () => button("Approve").click())
  await act(async () => settle(10))
  // The person answered in words; the list lets the ask go.
  act(() =>
    source.emit({
      kind: "session",
      session: summary("c", "desktop", 110, "idle", { revision: 2 }),
    }),
  )
  await act(async () => settle(10))
  sayConversation(source, false, { mode: "full", modes: ["ask", "auto", "full"] })
  await act(async () => settle(10))
  expect(source.calls.filter((call) => call[0] === "send")).toEqual([])
})
