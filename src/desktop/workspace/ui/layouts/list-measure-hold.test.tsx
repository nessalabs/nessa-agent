// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { act } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { expect, it } from "vitest"
import {
  closePane,
  fitToWindow,
  loadWorkspace,
  newSession,
} from "../../adapters/store/commands"
import { selectColumnCount } from "../../adapters/store/selectors"
import { controlledAnimationFrames, fakeSource, settle, testStore } from "../../testing"
import { ListMeasureHold } from "./column-paint"

it.each(["split beside", "close stacked pane"] as const)(
  "keeps the header outside the controls while the body is held: %s",
  async (action) => {
    ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
      true
    const store = testStore(fakeSource(), () => ({ width: 720, height: 684, spare: 0 }))
    await store.dispatch(loadWorkspace())
    await settle()
    if (action === "close stacked pane") {
      store.dispatch(newSession({ beside: "right" }))
      store.dispatch(newSession({ beside: "bottom" }))
    }
    store.dispatch(
      fitToWindow({ windowWidth: 1000, sidebarWidth: 200, sessionListWidth: 260 }),
    )
    const frames = controlledAnimationFrames()
    const sheet = document.createElement("style")
    const sheetPath = "../session-list/session-list.css"
    sheet.textContent = readFileSync(new URL(sheetPath, import.meta.url), "utf8")
    document.head.append(sheet)
    const workspace = document.createElement("div")
    workspace.className = "workspace"
    workspace.dataset.sidebar = action === "split beside" ? "open" : "closed"
    workspace.dataset.sidebarRow = workspace.dataset.sidebar
    workspace.dataset.list = "open"
    workspace.innerHTML =
      '<section class="workspace-list"><div class="workspace-list-inner"><div class="desktop-column-bar"><svg></svg><h2>desktop-app</h2></div></div></section><div class="hold-owner"></div>'
    document.body.append(workspace)
    const owner = workspace.querySelector(".hold-owner")
    const bar = workspace.querySelector(".desktop-column-bar")
    const inner = workspace.querySelector(".workspace-list-inner")
    if (!owner || !bar || !inner) throw new Error("list hold fixture is incomplete")
    const root = createRoot(owner)
    const bodyPadding = getComputedStyle(inner).paddingLeft
    try {
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ListMeasureHold root={{ current: workspace }} />
          </Provider>,
        ),
      )
      await act(async () => {
        if (action === "split beside") {
          store.dispatch(newSession({ beside: "right" }))
          store.dispatch(
            fitToWindow({ windowWidth: 1000, sidebarWidth: 200, sessionListWidth: 260 }),
          )
        } else store.dispatch(closePane())
      })
      expect(selectColumnCount(store.getState())).toBe(2)
      // These marks follow FoldMark and the grid's published frame order.
      // jsdom checks CSS destinations; actual animation geometry is a
      // separate browser acceptance gate, not simulated layout evidence.
      for (let frame = 1; frame < 5; frame++) {
        if (action === "split beside") {
          workspace.toggleAttribute("data-spare-held", frame < 3)
          if (frame === 2) workspace.dataset.sidebar = "closed"
          if (frame === 3) workspace.dataset.sidebarRow = "closed"
        }
        await act(async () => frames.runFrame())
        expect(workspace.hasAttribute("data-list-held")).toBe(true)
        // Preserve the existing body-hold destination; only the header
        // is independent of this hold.
        expect(getComputedStyle(inner).paddingLeft).toBe(
          action === "split beside" ? bodyPadding : "0",
        )
        const atEdge =
          workspace.dataset.sidebar === "closed" &&
          !workspace.hasAttribute("data-spare-held")
        expect(getComputedStyle(bar).paddingLeft).toBe(
          atEdge ? "var(--desktop-titlebar-safe-start)" : "12px",
        )
      }
      await act(async () => frames.runFrame())
      expect(workspace.hasAttribute("data-list-held")).toBe(false)
      expect(getComputedStyle(inner).paddingLeft).toBe("var(--desktop-gutter)")
      expect(getComputedStyle(bar).paddingLeft).toBe("var(--desktop-titlebar-safe-start)")
    } finally {
      await act(async () => root.unmount())
      workspace.remove()
      sheet.remove()
      frames.restore()
    }
  },
)
