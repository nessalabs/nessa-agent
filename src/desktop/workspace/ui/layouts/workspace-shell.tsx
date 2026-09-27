/**
 * What both layouts are made of besides their parts: the window's root with
 * its light, the providers every part reads (frame, listed channel, drag,
 * motion), and the keyboard's shared commands. A layout names its variant,
 * its keys and its columns, and places its parts inside.
 */
import {
  useMemo,
  useRef,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react"
import type { HostKind } from "../../../../host/features"
import { useThemePreference } from "../../../adapters/theme-preference"
import {
  closePane,
  focusPane,
  newSession,
  nudgePane,
  toggleSidebar,
} from "../../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import { selectFocusedPaneKey, selectPanes } from "../../adapters/store/selectors"
import { DragProvider } from "../../adapters/dom/drag"
import { FlipScope } from "../../adapters/dom/flip"
import { reducedMotion } from "../../adapters/dom/motion"
import { labelOf, useKeyBindings, type Binding } from "../../adapters/dom/shortcuts"
import {
  layoutShape,
  panesOf,
  type Direction,
  type PaneKey,
} from "../../model/pane-layout"
import type { PaneRoom } from "../../model/pane-sizing"
import {
  ListedChannelProvider,
  WorkspaceFrameProvider,
  type ShortcutCommand,
  type WorkspaceFrame,
} from "../workspace-frame"
import "../chrome/chrome.css"

/** Measures a pane for a split: its size, and what the open sidebar would give up. */
function measureRoom(
  root: HTMLElement | null,
  pane: PaneKey,
  sidebarSpare: number,
): PaneRoom | undefined {
  const box = root?.querySelector(`[data-pane-key="${pane}"]`)?.getBoundingClientRect()
  return box ? { width: box.width, height: box.height, spare: sidebarSpare } : undefined
}

/** The frame a layout lends its parts; its functions keep their identity. */
export function useLayoutFrame({
  root,
  bindings,
  sidebarSpare,
  openSwitcher,
}: {
  root: RefObject<HTMLElement | null>
  bindings: readonly Binding<ShortcutCommand>[]
  /** The sidebar's drawn width and gutter while it is open, else zero. */
  sidebarSpare: number
  openSwitcher?: (mode: "open" | "split") => void
}): WorkspaceFrame {
  const latest = useRef({ sidebarSpare, openSwitcher })
  latest.current = { sidebarSpare, openSwitcher }
  const hasSwitcher = openSwitcher !== undefined
  return useMemo<WorkspaceFrame>(
    () => ({
      shortcut: (command) => labelOf(bindings, command),
      roomOf: (pane) => measureRoom(root.current, pane, latest.current.sidebarSpare),
      openSwitcher: hasSwitcher
        ? (mode) => latest.current.openSwitcher?.(mode)
        : undefined,
      showRow: (sessionId) =>
        requestAnimationFrame(() => {
          const scope = root.current
          const selector = `[data-session-row="${CSS.escape(sessionId)}"]`
          const row =
            scope?.querySelector<HTMLElement>(`.workspace-sidebar ${selector}`) ??
            scope?.querySelector<HTMLElement>(selector)
          row?.scrollIntoView({
            block: "nearest",
            behavior: reducedMotion() ? "auto" : "smooth",
          })
          row?.focus({ preventScroll: true })
        }),
    }),
    [bindings, root, hasSwitcher],
  )
}

const moves: Partial<Record<ShortcutCommand, Direction>> = {
  moveLeft: "left",
  moveRight: "right",
  moveUp: "up",
  moveDown: "down",
}

/**
 * Runs the window's keys: the commands both layouts share, and `own` for the
 * ones a layout answers itself. A handler returning false leaves the key alone.
 */
export function useWorkspaceKeys(
  bindings: readonly Binding<ShortcutCommand>[],
  frame: WorkspaceFrame,
  own: Partial<Record<ShortcutCommand, () => boolean | void>>,
  /** While true, only `own` commands run: something modal has the keyboard. */
  paused = false,
) {
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
  const focused = () => selectFocusedPaneKey(store.getState())
  const beside = (side: "right" | "bottom") => {
    const pane = focused()
    dispatch(
      newSession({ beside: side, room: pane === null ? undefined : frame.roomOf(pane) }),
    )
  }
  // Focusing a pane by number, or its neighbour, puts the caret in its composer.
  const focusAt = (index: number) => {
    const panes = selectPanes(store.getState())
    const pane = panes ? panesOf(panes)[index] : undefined
    if (!pane) return false
    dispatch(focusPane({ pane: pane.key }))
    requestAnimationFrame(() =>
      document
        .querySelector<HTMLElement>(
          `[data-pane-key="${pane.key}"] textarea, [data-pane-key="${pane.key}"] [data-pane-focus]`,
        )
        ?.focus(),
    )
  }
  const focusBeside = (step: -1 | 1) => {
    const panes = selectPanes(store.getState())
    if (!panes) return false
    const order = panesOf(panes)
    const at = order.findIndex((pane) => pane.key === panes.focused)
    return focusAt(Math.min(Math.max(at + step, 0), order.length - 1))
  }
  useKeyBindings(bindings, (command) => {
    const handler = own[command]
    if (handler) return handler()
    if (paused) return false
    const direction = moves[command]
    if (direction) {
      const pane = focused()
      if (pane !== null) dispatch(nudgePane({ pane, direction }))
      return
    }
    switch (command) {
      case "toggleSidebar":
        dispatch(toggleSidebar())
        return
      case "newSession":
        dispatch(newSession())
        return
      case "newSessionBeside":
      case "splitRight":
        beside("right")
        return
      case "splitDown":
        beside("bottom")
        return
      case "closePane":
        dispatch(closePane())
        return
      case "focusPane1":
      case "focusPane2":
      case "focusPane3":
      case "focusPane4":
        return focusAt(Number(command.slice(-1)) - 1)
      case "focusPrevious":
        return focusBeside(-1)
      case "focusNext":
        return focusBeside(1)
      default:
        return false
    }
  })
}

/** The shape of what moves on screen: the panes' arrangement and which columns are open. */
export function useMotionShape(...columns: boolean[]): string {
  const panes = useWorkspaceSelector((state) => {
    const layout = selectPanes(state)
    return layout ? layoutShape(layout) : ""
  })
  return `${panes}|${columns.join(",")}`
}

/**
 * The window's root for a layout: its surface and light, and the providers
 * its parts read. `data-*` on the root carry what the stylesheet lays out by.
 */
export function WorkspaceShell({
  layout,
  hostKind,
  browserSurface,
  root,
  frame,
  listedChannel,
  shape,
  data,
  style,
  children,
}: {
  layout: "columns" | "sidebar"
  hostKind: HostKind
  browserSurface: boolean
  root: RefObject<HTMLDivElement | null>
  frame: WorkspaceFrame
  listedChannel: string | null
  shape: string
  data: Record<`data-${string}`, string | boolean | undefined>
  style: CSSProperties
  children: ReactNode
}) {
  const [theme] = useThemePreference()
  return (
    <WorkspaceFrameProvider value={frame}>
      <ListedChannelProvider value={listedChannel}>
        <DragProvider root={root}>
          <FlipScope shape={shape} root={root}>
            <div
              ref={root}
              className="workspace"
              data-workspace
              data-layout={layout}
              data-host={hostKind}
              data-surface={browserSurface ? "browser" : "window"}
              data-desktop-theme={theme}
              {...data}
              style={style}
            >
              <div className="desktop-ambient" aria-hidden="true">
                <span className="desktop-grain" />
              </div>
              {children}
            </div>
          </FlipScope>
        </DragProvider>
      </ListedChannelProvider>
    </WorkspaceFrameProvider>
  )
}
