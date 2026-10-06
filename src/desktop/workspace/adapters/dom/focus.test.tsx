// @vitest-environment jsdom
/**
 * The caret follows the focused pane: into its composer whenever another
 * pane takes focus or what held the caret went away, and never out of a
 * list the person is walking, or a dialog.
 */
import { act, useRef } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import {
  closePane,
  focusPane,
  loadWorkspace,
  newSession,
  openSession,
  showContent,
} from "../store/commands"
import { shallowEqual } from "react-redux"
import { useWorkspaceSelector, useWorkspaceStore } from "../store/hooks"
import { workspaceActions } from "../store/slice"
import { selectFocusedPaneKey, selectShownSessionIds } from "../store/selectors"
import { panesOf } from "../../../split-panes/model/pane-layout"
import { emptyTranscript, type Transcript } from "../../model/transcript"
import {
  controlledAnimationFrames,
  flushAnimationFrames,
  settle,
  testStore,
  type AnimationFrames,
} from "../../testing"
import { focusedPaneAttribute, useFocusFollowsPane } from "./focus"

/** Panes as the page draws them, each with a composer, and a list beside them. */
function Page() {
  const root = useRef<HTMLDivElement>(null)
  const store = useWorkspaceStore()
  useFocusFollowsPane(store, root)
  const focused = useWorkspaceSelector(selectFocusedPaneKey)
  const shown = useWorkspaceSelector(selectShownSessionIds)
  const panes = useWorkspaceSelector((state) => state.workspace.panes)
  const asking = useWorkspaceSelector(
    (state) =>
      Object.values(state.workspace.transcripts)
        .filter((transcript) => transcript.approval)
        .map((transcript) => transcript.sessionId),
    shallowEqual,
  )
  return (
    <div ref={root}>
      <button type="button" data-list-row>
        A row in a list
      </button>
      <div className="split-panes-grid" data-split-grid>
        {(panes ? panesOf(panes) : []).map((pane, index) => (
          <article
            key={pane.key}
            data-pane-key={pane.key}
            {...{ [focusedPaneAttribute]: pane.key === focused || undefined }}
          >
            {/* An approval, which goes once it is answered. */}
            {asking.includes(shown[index]) ? (
              <button type="button">Approve</button>
            ) : null}
            <form className="desktop-composer">
              <textarea aria-label={`Message ${pane.key}`} />
            </form>
          </article>
        ))}
      </div>
    </div>
  )
}

let root: Root
let host: HTMLDivElement
let animation: AnimationFrames

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  animation = controlledAnimationFrames()
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  animation.restore()
})

/** The frames the caret's landing asked for, run because the test says so. */
const frames = () => flushAnimationFrames(animation, act)
const caretIn = () => document.activeElement?.getAttribute("aria-label")

async function page() {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  await settle()
  await act(async () =>
    root.render(
      <Provider store={store}>
        <Page />
      </Provider>,
    ),
  )
  return store
}

it("puts the caret in the new pane's composer after a split, ⌘N and ⌘W", async () => {
  const store = await page()
  host.querySelector("textarea")?.focus()
  await act(async () => void store.dispatch(newSession({ beside: "right" })))
  await frames()
  const second = store.getState().workspace.panes?.focused
  expect(caretIn()).toBe(`Message ${second}`)
  await act(async () => void store.dispatch(focusPane({ pane: 1 })))
  await frames()
  expect(caretIn()).toBe("Message 1")
  await act(async () => void store.dispatch(closePane()))
  await frames()
  expect(caretIn()).toBe(`Message ${second}`)
})

it("follows a pane taking focus even from a list, and leaves a list walked in one pane alone", async () => {
  const store = await page()
  await act(async () => void store.dispatch(newSession({ beside: "right" })))
  await frames()
  const row = host.querySelector<HTMLElement>("[data-list-row]")
  row?.focus()
  // The list opens another session in the focused pane: the caret stays in the list.
  await act(async () => void store.dispatch(openSession({ sessionId: "c" })))
  await frames()
  expect(document.activeElement).toBe(row)
  // Another pane takes focus (⌘1): the caret goes with it.
  await act(async () => void store.dispatch(focusPane({ pane: 1 })))
  await frames()
  expect(caretIn()).toBe("Message 1")
})

it("picks the caret up when what held it in the pane goes away, as an answered approval", async () => {
  const store = await page()
  const asked = (approval: Transcript["approval"], revision: number) =>
    act(
      async () =>
        void store.dispatch(
          workspaceActions.updateReceived({
            update: {
              kind: "transcript",
              transcript: { ...emptyTranscript("a"), approval, revision },
            },
          }),
        ),
    )
  await asked(
    { id: "ap", command: "ls", reason: "look", origin: { kind: "agent" }, options: [] },
    5,
  )
  host.querySelector<HTMLButtonElement>("[data-pane-key] button")?.focus()
  expect(document.activeElement?.textContent).toBe("Approve")
  // Answered: the conversation no longer asks, and the button goes. The pane is as it was.
  await asked(null, 6)
  await frames()
  expect(caretIn()).toBe("Message 1")
})

it("leaves the caret in a dialog", async () => {
  const store = await page()
  const dialog = document.createElement("div")
  dialog.setAttribute("role", "dialog")
  const field = document.createElement("input")
  dialog.append(field)
  document.body.append(dialog)
  field.focus()
  await act(async () => void store.dispatch(newSession({ beside: "right" })))
  await frames()
  expect(document.activeElement).toBe(field)
  dialog.remove()
})

it("puts the caret in the focused pane's composer when the panes come back from the overview, by a command that changes nothing else", async () => {
  const store = await page()
  const focused = store.getState().workspace.panes?.focused
  await act(async () => void store.dispatch(showContent({ content: "agents" })))
  await frames()
  // In the overview, the keyboard is its own: nothing here takes it.
  const row = host.querySelector<HTMLElement>("[data-list-row]")
  row?.focus()
  row?.remove()
  expect(document.activeElement).toBe(document.body)
  // ⌘ and the focused pane's number: it goes to the pane it is already on.
  await act(async () => void store.dispatch(focusPane({ pane: focused ?? 1 })))
  await frames()
  expect(store.getState().workspace.content).toBe("panes")
  expect(caretIn()).toBe(`Message ${focused}`)
})
