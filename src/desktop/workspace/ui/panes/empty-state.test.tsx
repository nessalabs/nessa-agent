// @vitest-environment jsdom
/**
 * The workspace with nothing to show (#419): an index the window could not
 * read says why — signed out, or no answer from the local server — and Try
 * Again reads it again. (That the desktop app draws this rather than the
 * sample is `gateway-states.mjs`'s to check, in a browser.)
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { loadWorkspace } from "../../adapters/store/commands"
import { selectFailure } from "../../adapters/store/selectors"
import { fakeSource, settle, testStore } from "../../testing"
import {
  startupRefusalSentence,
  wrongStageSentence,
} from "../../../../host/startup-refusals"
import { EmptyWorkspace } from "./empty-state"

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

async function shown(reason: "signed-out" | "unavailable" | undefined) {
  const source = fakeSource()
  source.refuse("index", reason)
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  await act(async () =>
    root.render(
      <Provider store={store}>
        <EmptyWorkspace failure={selectFailure(store.getState())} />
      </Provider>,
    ),
  )
  return { source, store }
}

it("S8: signed out, it says the window is not signed in", async () => {
  await shown("signed-out")
  expect(host.querySelector("[role=status] p")?.textContent).toBe(
    "This window isn’t signed in to the local server.",
  )
})

it("S8: with no answer, it says what it could not read and from where", async () => {
  await shown("unavailable")
  expect(host.querySelector("[role=status] p")?.textContent).toBe(
    "Nessa couldn’t read the local server’s conversations just now.",
  )
})

it("S8: Try Again reads the index again, and a read that answers opens the workspace", async () => {
  const { source, store } = await shown("signed-out")
  source.refuse("index", undefined)
  const asked = source.calls.filter(([method]) => method === "index").length
  await act(async () => {
    host.querySelector("button")?.click()
    await settle()
  })
  expect(source.calls.filter(([method]) => method === "index")).toHaveLength(asked + 1)
  expect(store.getState().workspace.status).toBe("ready")
})

it("a stage mismatch names both stages", async () => {
  const store = testStore(fakeSource())
  await act(async () =>
    root.render(
      <Provider store={store}>
        <EmptyWorkspace
          failure="wrong-stage"
          stages={{ bundle: "dev", requested: "prod" }}
        />
      </Provider>,
    ),
  )
  expect(host.querySelector("[role=status] p")?.textContent).toBe(
    wrongStageSentence({ bundle: "dev", requested: "prod" }),
  )
})

it("a server that is not answering says so", async () => {
  const store = testStore(fakeSource())
  await act(async () =>
    root.render(
      <Provider store={store}>
        <EmptyWorkspace failure="not-listening" />
      </Provider>,
    ),
  )
  expect(host.querySelector("[role=status] p")?.textContent).toBe(
    startupRefusalSentence("not-listening"),
  )
})

it("a workspace that was read shows nothing here", async () => {
  await shown(undefined)
  expect(host.innerHTML).toBe("")
})
