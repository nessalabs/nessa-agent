// @vitest-environment jsdom
/**
 * The folded sidebar's reach while a reveal from the edge comes and goes
 * (`use-edge-peek.ts`, `adapters/dom/parked.ts`).
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useEdgePeek, type EdgePeekControls } from "../../../adapters/use-edge-peek"
import { loadWorkspace } from "../../adapters/store/commands"
import { testStore } from "../../testing"
import { SidebarPeekProvider } from "../workspace-frame"
import { OverviewQuietProvider } from "./overview-quiet"
import { SourceList } from "./source-list"

let host: HTMLDivElement
let react: Root
let peek: EdgePeekControls

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  react = createRoot(host)
})

afterEach(() => {
  act(() => react.unmount())
  host.remove()
  vi.useRealTimers()
})

function Folded() {
  peek = useEdgePeek(true, false)
  return (
    <SidebarPeekProvider value={peek}>
      <SourceList variant="channels" open={false} />
    </SidebarPeekProvider>
  )
}

const free = { buttons: 0 }
const wait = (ms: number) => act(() => void vi.advanceTimersByTime(ms))

it("reveals again after Escape dismissed a reveal that held focus", async () => {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  vi.useFakeTimers()
  act(() =>
    react.render(
      <Provider store={store}>
        <OverviewQuietProvider store={store} root={{ current: host }}>
          <Folded />
        </OverviewQuietProvider>
      </Provider>,
    ),
  )
  act(() => peek.enter(free))
  await wait(200)
  expect(peek.shown).toBe(true)
  const aside = host.querySelector<HTMLElement>("#workspace-sidebar")
  const row = aside?.querySelector<HTMLElement>("button")
  if (!aside || !row) throw new Error("no sidebar row")
  // Focus that holds the reveal, as Tab gives it.
  act(() => {
    row.focus()
    peek.holders.onFocus({ target: row })
  })
  act(() => peek.leave(free))
  await wait(400)
  expect(peek.shown).toBe(true)
  act(() => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))
  })
  expect(peek.shown).toBe(false)
  // The fold took focus out of it, and the peek heard it leave.
  expect(aside.contains(document.activeElement)).toBe(false)
  act(() => peek.enter(free))
  await wait(200)
  expect(peek.shown).toBe(true)
})
