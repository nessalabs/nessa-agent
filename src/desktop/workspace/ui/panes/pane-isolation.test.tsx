// @vitest-environment jsdom
/**
 * ADR 238's promise, measured: a pane renders for its own session and
 * nothing else. Four real panes over a real store, each in a React Profiler;
 * a reply streaming into one, another's status changing, and typing in a
 * composer each render exactly the pane they concern.
 */
import { act, StrictMode, Profiler, useRef, type ReactNode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { focusPane, loadWorkspace, openBeside } from "../../adapters/store/commands"
import { paneFrame } from "../../../split-panes/testing"
import { placements as placementsOf } from "../../../split-panes/model/pane-sizing"
import {
  selectFocusedSessionId,
  selectPanes,
  selectPaneSession,
} from "../../adapters/store/selectors"
import { workspaceActions } from "../../adapters/store/slice"
import { ClockProvider } from "../../adapters/dom/clock"
import { emptyTranscript } from "../../model/transcript"
import { fakeSource, settle, summary, testStore } from "../../testing"
import { Pane } from "./pane"

/**
 * What a pane's content renders: the home and the conversation it hands its
 * callbacks to, counted by session. Wrapped in `memo` exactly as the real
 * ones are, so a count only rises when the pane hands them something new.
 */
const contentRenders = vi.hoisted(() => new Map<string, number>())
vi.mock("./conversation", async (load) => {
  const { memo, createElement } = await import("react")
  const real = await load<typeof import("./conversation")>()
  const counted = <P extends { sessionId: string }>(
    name: string,
    Inner: (props: P) => ReturnType<typeof createElement>,
  ) =>
    memo(function Counted(props: P) {
      const key = `${name}:${props.sessionId}`
      contentRenders.set(key, (contentRenders.get(key) ?? 0) + 1)
      return createElement(Inner as never, props as never)
    })
  return {
    ...real,
    Conversation: counted("Conversation", real.Conversation as never),
    PaneHome: counted("PaneHome", real.PaneHome as never),
  }
})

class Observer {
  observe() {}
  unobserve() {}
  disconnect() {}
}

let root: Root
let host: HTMLDivElement
const renders = new Map<string, number>()

function Frame({ children }: { children: ReactNode }) {
  const workspace = useRef<HTMLDivElement>(null)
  return <div ref={workspace}>{children}</div>
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer })
  // jsdom lays nothing out, so it has nothing to scroll.
  Element.prototype.scrollTo ??= () => {}
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
  renders.clear()
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

async function fourPanes() {
  const source = fakeSource()
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  for (const sessionId of ["b", "c", "d"]) store.dispatch(openBeside({ sessionId }))
  await settle()
  const panes = selectPanes(store.getState())
  const placements = panes ? placementsOf(panes.columns).panes : []
  expect(placements).toHaveLength(4)
  const sessionOf = (key: number) => selectPaneSession(store.getState(), key) ?? ""
  await act(async () => {
    root.render(
      <StrictMode>
        <Provider store={store}>
          <ClockProvider now={() => 1000}>
            <Frame>
              {placements.map((placement) => (
                <Profiler
                  key={placement.key}
                  id={sessionOf(placement.key)}
                  onRender={(id) => renders.set(id, (renders.get(id) ?? 0) + 1)}
                >
                  <Pane placement={placement} frame={paneFrame(placement)} multi />
                </Profiler>
              ))}
            </Frame>
          </ClockProvider>
        </Provider>
      </StrictMode>,
    )
  })
  // Each pane fills in a frame after its shell; let every one land.
  await act(async () => new Promise((resolve) => setTimeout(resolve, 50)))
  renders.clear()
  contentRenders.clear()
  return { store, source }
}

const rendered = () => [...renders.keys()].sort()

describe("a pane renders for its own session only", () => {
  it("renders one pane for a reply streaming into it", async () => {
    const { store } = await fourPanes()
    const base = store.getState().workspace.transcripts.c ?? emptyTranscript("c")
    for (const [index, text] of ["On", "On it.", "On it. Reading"].entries()) {
      await act(async () => {
        store.dispatch(
          workspaceActions.updateReceived({
            update: {
              kind: "transcript",
              transcript: {
                ...base,
                revision: base.revision + index + 1,
                messages: [
                  { id: "reply", role: "agent", at: 1, parts: [{ kind: "text", text }] },
                ],
              },
            },
          }),
        )
      })
    }
    expect(rendered()).toEqual(["c"])
    // Nor does the layout change: the grid that places the panes stays as it was.
    expect(host.textContent).toContain("On it. Reading")
  })

  it("renders one pane when another session's status changes", async () => {
    const { store } = await fourPanes()
    await act(async () => {
      store.dispatch(
        workspaceActions.updateReceived({
          update: {
            kind: "session",
            session: summary("d", "gateway", 999, "running", { revision: 2 }),
          },
        }),
      )
    })
    expect(rendered()).toEqual(["d"])
  })

  it("renders the two panes focus moves between, and no other", async () => {
    const { store } = await fourPanes()
    const panes = store.getState().workspace.panes
    const target = panes?.columns[0].panes[0]
    const was = selectFocusedSessionId(store.getState())
    await act(async () => {
      if (target) store.dispatch(focusPane({ pane: target.key }))
    })
    expect(rendered()).toEqual([target?.item, was].sort())
    // The two panes render their headers; what they show is handed nothing new.
    expect([...contentRenders.keys()]).toEqual([])
  })

  it("keeps the columns, which the grid renders from, when content changes", async () => {
    const { store } = await fourPanes()
    const before = selectPanes(store.getState())?.columns
    await act(async () => {
      store.dispatch(
        workspaceActions.updateReceived({
          update: { kind: "transcript", transcript: emptyTranscript("b") },
        }),
      )
    })
    expect(selectPanes(store.getState())?.columns).toBe(before)
  })

  it("renders one pane while its composer is typed in", async () => {
    await fourPanes()
    const field = host.querySelector<HTMLTextAreaElement>(
      '[data-pane-key] [aria-label="Message"]',
    )
    expect(field).not.toBeNull()
    const pane = field?.closest<HTMLElement>("[data-pane-key]")
    const setValue = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      "value",
    )?.set
    for (const text of ["S", "Sk", "Ske"]) {
      await act(async () => {
        setValue?.call(field, text)
        field?.dispatchEvent(new Event("input", { bubbles: true }))
      })
    }
    const owner = [...renders.keys()]
    expect(owner).toHaveLength(1)
    expect(pane?.getAttribute("aria-label")).toBe(`Session ${owner[0]}`)
  })
})

describe("a pane's header among several", () => {
  it("carries the pane, and nothing in it moves the window", async () => {
    await fourPanes()
    const headers = [...host.querySelectorAll(".workspace-pane-header")]
    expect(headers).toHaveLength(4)
    for (const header of headers) {
      expect(header.hasAttribute("data-drag-pane")).toBe(true)
      expect(header.closest("[data-tauri-drag-region]")).toBeNull()
      expect(header.querySelector("[data-tauri-drag-region]")).toBeNull()
    }
  })
})
