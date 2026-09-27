/**
 * Drag and drop in the workspace: a session carried from a list onto a pane,
 * or a pane carried by its header onto another. What is being carried is DOM
 * state — it lives as long as the pointer holds it — so it is kept here, in a
 * small store beside the tree, not in the product store. Panes subscribe to
 * the one fact they need ("am I the one lifted?"), and the workspace root is
 * marked `data-dragging` directly, so starting a drag renders nothing else.
 */
import {
  createContext,
  useContext,
  useState,
  useSyncExternalStore,
  type DragEvent,
  type ReactNode,
  type RefObject,
} from "react"
import type { PaneKey } from "../../model/pane-layout"

export const sessionDragType = "application/x-nessa-session"
export const paneDragType = "application/x-nessa-pane"

export type Carried =
  | { readonly kind: "session"; readonly sessionId: string }
  | { readonly kind: "pane"; readonly pane: PaneKey }
  | null

class DragStore {
  private carried: Carried = null
  private readonly listeners = new Set<() => void>()

  constructor(private readonly root: RefObject<HTMLElement | null>) {}

  get = (): Carried => this.carried

  set(next: Carried) {
    if (next === this.carried) return
    // However the drag ends — dropped anywhere, cancelled, or its source row
    // gone from the page, which then never hears its own dragend — the window
    // lets go: after a drop has been handled (the bubbling phase, so the pane
    // that takes it still sees what was carried), on dragend, or at the next
    // press, which cannot happen while a drag is in progress.
    if (next && !this.carried) {
      const events = ["drop", "dragend", "pointerdown"] as const
      const release = () => {
        for (const type of events) window.removeEventListener(type, release)
        this.set(null)
      }
      for (const type of events) window.addEventListener(type, release)
    }
    this.carried = next
    const root = this.root.current
    if (root) {
      if (next) root.dataset.dragging = next.kind
      else delete root.dataset.dragging
    }
    this.listeners.forEach((listener) => listener())
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }
}

const DragContext = createContext<DragStore | null>(null)

export function DragProvider({
  root,
  children,
}: {
  /** The workspace's root, marked while something is carried. */
  root: RefObject<HTMLElement | null>
  children: ReactNode
}) {
  const [store] = useState(() => new DragStore(root))
  return <DragContext.Provider value={store}>{children}</DragContext.Provider>
}

function useDragStore(): DragStore {
  const store = useContext(DragContext)
  if (!store) throw new Error("useDragStore needs a DragProvider")
  return store
}

/** The pane being carried, if one is. */
export function useCarriedPane(): PaneKey | null {
  const store = useDragStore()
  return useSyncExternalStore(store.subscribe, () => {
    const carried = store.get()
    return carried?.kind === "pane" ? carried.pane : null
  })
}

/**
 * A compact pill for the drag, in place of the browser's translucent copy of
 * the whole row, which overlaps the rows around it: the session's agent tile
 * and its title.
 */
function setDragImage(event: DragEvent<HTMLElement>, title: string) {
  const source = event.currentTarget
  const ghost = document.createElement("div")
  ghost.className = "workspace-drag-image"
  const tile = source.querySelector(".workspace-agent-tile")?.cloneNode(true)
  if (tile) ghost.append(tile)
  const label = document.createElement("span")
  label.textContent = title
  ghost.append(label)
  ;(source.closest("[data-workspace]") ?? document.body).append(ghost)
  event.dataTransfer.setDragImage(ghost, 18, 17)
  // The image is captured synchronously; the node is only needed this frame.
  requestAnimationFrame(() => ghost.remove())
}

/** Starts carrying a session from a row; the row stays put, a little dimmed. */
export function useSessionDrag() {
  const store = useDragStore()
  return (event: DragEvent<HTMLElement>, sessionId: string, title: string) => {
    event.dataTransfer.setData(sessionDragType, sessionId)
    event.dataTransfer.setData("text/plain", title)
    event.dataTransfer.effectAllowed = "copyMove"
    setDragImage(event, title)
    const row = event.currentTarget
    row.dataset.dragging = ""
    store.set({ kind: "session", sessionId })
    row.addEventListener(
      "dragend",
      () => {
        delete row.dataset.dragging
        store.set(null)
      },
      { once: true },
    )
  }
}

/** Starts carrying a pane by its header, and lets go when the drag ends. */
export function usePaneDrag() {
  const store = useDragStore()
  return {
    start(event: DragEvent<HTMLElement>, pane: PaneKey, title: string) {
      event.dataTransfer.setData(paneDragType, String(pane))
      event.dataTransfer.effectAllowed = "move"
      setDragImage(event, title)
      store.set({ kind: "pane", pane })
    },
    end() {
      store.set(null)
    },
    carried: store.get,
  }
}
