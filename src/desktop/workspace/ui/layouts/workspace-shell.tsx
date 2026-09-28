/**
 * The workspace window, whichever layout it is in. Everything the two
 * layouts share lives here, once: the window's root and its light; the
 * titlebar's controls (sidebar toggle, Back, Forward); the keyboard
 * (`workspaceShortcuts`, one map); the quick switcher (⌘K, ⌘\); the pane grid
 * and everything in a pane; focus following the focused pane; drag and drop;
 * the folded sidebar's reveal from the edge; fitting the side columns to the
 * window; the Agents overview's layer over the list and the panes
 * (`ui/overview/`). A layout (`three-columns.tsx`, `sessions-in-sidebar.tsx`)
 * says only how the sidebar region is composed — which sidebar, and whether a
 * session list stands beside it — as a `SidebarRegion`.
 */
import {
  memo,
  useCallback,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type RefObject,
} from "react"
import type { HostKind } from "../../../../host/features"
import { reducedMotion } from "../../../adapters/motion-preference"
import { useThemePreference } from "../../../adapters/theme-preference"
import { useEdgePeek } from "../../../adapters/use-edge-peek"
import { useWindowWidth } from "../../../adapters/window-width"
import { draggedEdge } from "../../../model/side-column"
import { EdgePeekStrip } from "../../../ui/edge-peek-strip"
import { HistoryButtons } from "../../../ui/history-buttons"
import { useWorkspaceDrag } from "../../adapters/dom/drag"
import { FlipScope } from "../../adapters/dom/flip"
import { useFocusFollowsPane } from "../../adapters/dom/focus"
import { labelOf, useKeyBindings, type Binding } from "../../adapters/dom/shortcuts"
import { useFitOnResize } from "../../adapters/dom/window-width"
import {
  canOpenBeside,
  closePane,
  fitToWindow,
  focusPane,
  newSession,
  nudgePane,
  openBeside,
  openChannel,
  openSession,
  resizeSessionList,
  resizeSidebar,
  sendMessage,
  showContent,
  toggleSessionList,
  toggleSidebar,
} from "../../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import {
  selectChrome,
  selectColumnCount,
  selectFocusedChannel,
  selectFocusedPaneKey,
  selectOverviewOpen,
  selectPanes,
  selectSessionListOpen,
  selectSidebarOpen,
  selectView,
} from "../../adapters/store/selectors"
import {
  layoutShape,
  panesOf,
  type Direction,
} from "../../../split-panes/model/pane-layout"
import type { SwitcherRow } from "../../model/session-search"
import { columnWidth, sessionListLimits, type ColumnLimits } from "../../model/window-fit"
import { IconButton } from "../chrome/icon-button"
import { ResizeEdge } from "../../../split-panes/ui/resize-edge"
import { WorkspaceTitlebar } from "../chrome/workspace-titlebar"
import { OverviewLayer } from "../overview/overview-layer"
import { PaneGrid } from "../panes/pane-grid"
import { QuickSwitcher, type SwitcherMode } from "../quick-switcher/quick-switcher"
import { SessionList } from "../session-list/session-list"
import { SourceList } from "../source-list/source-list"
import {
  ListedChannelProvider,
  SidebarPeekProvider,
  WorkspaceFrameProvider,
  type ShortcutCommand,
  type WorkspaceFrame,
} from "../workspace-frame"
import { workspaceShortcuts } from "./shortcuts"
import "../chrome/chrome.css"
import "./layouts.css"

/**
 * How a layout composes the window's sidebar region: the sidebar's variant —
 * channels to choose from, or channels that disclose their sessions — its
 * width until the person drags it and the limits it is held to, and whether
 * a session list stands beside it.
 */
export interface SidebarRegion {
  readonly sidebar: {
    readonly variant: "channels" | "tree"
    readonly defaultWidth: number
    readonly limits: ColumnLimits
  }
  readonly sessionList: boolean
}

/** The frame the window lends its parts; its functions keep their identity. */
function useWindowFrame(
  root: RefObject<HTMLElement | null>,
  bindings: readonly Binding<ShortcutCommand>[],
  openSwitcher: (mode: SwitcherMode) => void,
): WorkspaceFrame {
  const latest = useRef(openSwitcher)
  latest.current = openSwitcher
  return useMemo<WorkspaceFrame>(
    () => ({
      shortcut: (command) => labelOf(bindings, command),
      openSwitcher: (mode) => latest.current(mode),
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
    [bindings, root],
  )
}

const moves: Partial<Record<ShortcutCommand, Direction>> = {
  moveLeft: "left",
  moveRight: "right",
  moveUp: "up",
  moveDown: "down",
}

/**
 * Runs the window's keys: the commands every layout shares, and `own` for
 * the ones the window answers with its own parts (the switcher, the list). A
 * handler returning false leaves the key alone. Where the caret goes after
 * each is not the keys' business: it follows the focused pane
 * (`adapters/dom/focus.ts`).
 */
function useWorkspaceKeys(
  own: Partial<Record<ShortcutCommand, () => boolean | void>>,
  /** While true, only `own` commands run: something modal has the keyboard. */
  paused: boolean,
) {
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
  const focused = () => selectFocusedPaneKey(store.getState())
  const focusAt = (index: number) => {
    const panes = selectPanes(store.getState())
    const pane = panes ? panesOf(panes)[index] : undefined
    if (!pane) return false
    dispatch(focusPane({ pane: pane.key }))
  }
  const focusBeside = (step: -1 | 1) => {
    const panes = selectPanes(store.getState())
    if (!panes) return false
    const order = panesOf(panes)
    const at = order.findIndex((pane) => pane.key === panes.focused)
    return focusAt(Math.min(Math.max(at + step, 0), order.length - 1))
  }
  useKeyBindings(workspaceShortcuts, (command) => {
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
        dispatch(newSession({ beside: "right" }))
        return
      case "splitDown":
        dispatch(newSession({ beside: "bottom" }))
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
function useMotionShape(...columns: boolean[]): string {
  const panes = useWorkspaceSelector((state) => {
    const layout = selectPanes(state)
    return layout ? layoutShape(layout) : ""
  })
  return `${panes}|${columns.join(",")}`
}

/** A column's width as laid out — never as a slide draws it — which a drag of its edge starts from. */
const drawnWidth = (root: HTMLElement | null, selector: string) =>
  root?.querySelector<HTMLElement>(selector)?.offsetWidth ?? 0

export function WorkspaceShell({
  region,
  hostKind,
  browserSurface,
}: {
  region: SidebarRegion
  hostKind: HostKind
  browserSurface: boolean
}) {
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
  const root = useRef<HTMLDivElement>(null)
  const [theme] = useThemePreference()
  const chrome = useWorkspaceSelector(selectChrome)
  const sidebarOpen = useWorkspaceSelector(selectSidebarOpen)
  const listDrawn = useWorkspaceSelector(selectSessionListOpen)
  const listOpen = region.sessionList && listDrawn
  const view = useWorkspaceSelector(selectView)
  const columns = useWorkspaceSelector(selectColumnCount)
  const windowWidth = useWindowWidth()
  const sidebarWidth = columnWidth(
    chrome.sidebarWidth ?? region.sidebar.defaultWidth,
    region.sidebar.limits,
    windowWidth,
  )
  const listWidth = region.sessionList
    ? columnWidth(chrome.sessionListWidth, sessionListLimits, windowWidth)
    : 0
  useFitOnResize(windowWidth, columns, (width) =>
    dispatch(
      fitToWindow({ windowWidth: width, sidebarWidth, sessionListWidth: listWidth }),
    ),
  )

  const [switcher, setSwitcher] = useState<SwitcherMode | null>(null)
  const frame = useWindowFrame(root, workspaceShortcuts, setSwitcher)
  // While the switcher is up it has the keyboard, but for ⌘K, which closes it.
  useWorkspaceKeys(
    {
      switcher: () => setSwitcher((open) => (open ? null : "open")),
      // Offered as "beside" only where a pane could open there; else it is a jump.
      openBeside: () =>
        switcher ? false : setSwitcher(dispatch(canOpenBeside()) ? "split" : "open"),
      toggleSessionList: () => {
        if (!region.sessionList) return false
        dispatch(toggleSessionList())
      },
      // A place to go, as the sidebar's entry is: asked again, it stays; Escape in it leaves.
      showOverview: () => {
        if (switcher) return false
        dispatch(showContent({ content: "agents" }))
      },
      // Searching the list brings it back first; with no list, it is a jump.
      search: () => {
        if (switcher) return false
        if (!region.sessionList) return setSwitcher("open")
        dispatch(toggleSessionList({ open: true }))
        requestAnimationFrame(() =>
          root.current
            ?.querySelector<HTMLInputElement>(".workspace-search input")
            ?.focus(),
        )
      },
    },
    switcher !== null,
  )
  useFocusFollowsPane(store, root)
  useWorkspaceDrag(store, root)
  const peek = useEdgePeek(!sidebarOpen, sidebarOpen)
  // What fills the content region: the panes, or the Agents overview over them.
  const overviewShown = useWorkspaceSelector(selectOverviewOpen)
  const shape = useMotionShape(sidebarOpen, listOpen)

  // Beside where the room allows it, in the focused pane's place where not —
  // what ⌘-click does — so a pick never goes nowhere, and a typed message is
  // never lost.
  const pick = (row: SwitcherRow, asked: boolean) => {
    setSwitcher(null)
    const beside = asked && dispatch(canOpenBeside())
    if (row.kind === "session") {
      dispatch(
        beside
          ? openBeside({ sessionId: row.session.id })
          : openSession({ sessionId: row.session.id }),
      )
    } else if (row.kind === "channel") {
      dispatch(openChannel({ channelId: row.channel.id, beside }))
    } else {
      const draftId = dispatch(
        newSession({ channelId: row.channelId, beside: beside ? "right" : undefined }),
      )
      if (draftId && row.text)
        void dispatch(
          sendMessage({ sessionId: draftId, text: row.text, initiator: "person" }),
        )
    }
  }

  const compose = useCallback(() => dispatch(newSession()), [dispatch])
  const composeShortcut = frame.shortcut("newSession")
  // In a sidebar that lists sessions, compose sits at its head; with the
  // sidebar away it waits in the titlebar, by the toggle.
  const top = useMemo(
    () =>
      region.sidebar.variant === "tree" ? (
        <IconButton
          icon="newSession"
          label="New Session"
          shortcut={composeShortcut}
          onClick={compose}
        />
      ) : undefined,
    [region.sidebar.variant, compose, composeShortcut],
  )
  return (
    <WorkspaceFrameProvider value={frame}>
      <ListedChannelProvider value={listOpen ? view.channelId : null}>
        <SidebarPeekProvider value={peek}>
          <FlipScope shape={shape} root={root}>
            <div
              ref={root}
              className="workspace"
              data-workspace
              data-host={hostKind}
              data-surface={browserSurface ? "browser" : "window"}
              data-desktop-theme={theme}
              data-peek={(peek.shown && !peek.handedOff) || undefined}
              data-sidebar={sidebarOpen ? "open" : "closed"}
              data-list={region.sessionList ? (listOpen ? "open" : "closed") : undefined}
              data-panes-alone={(!sidebarOpen && !listOpen) || undefined}
              data-content={overviewShown ? "agents" : "panes"}
              style={
                {
                  "--workspace-sidebar-width": `${sidebarWidth}px`,
                  ...(region.sessionList
                    ? { "--workspace-list-width": `${listWidth}px` }
                    : {}),
                } as CSSProperties
              }
            >
              <div className="desktop-ambient" aria-hidden="true">
                <span className="desktop-grain" />
              </div>
              {sidebarOpen ? null : (
                <EdgePeekStrip
                  peek={peek}
                  onDragOut={() => dispatch(toggleSidebar({ open: true }))}
                />
              )}
              <WorkspaceTitlebar>
                <IconButton
                  icon="sidebar"
                  label={`${sidebarOpen ? "Hide" : "Show"} Sidebar`}
                  shortcut={frame.shortcut("toggleSidebar")}
                  aria-expanded={sidebarOpen}
                  aria-controls="workspace-sidebar"
                  onClick={() => dispatch(toggleSidebar())}
                />
                <HistoryButtons className="workspace-icon-button" />
                {region.sessionList ? (
                  <IconButton
                    icon="sessionList"
                    label={`${listOpen ? "Hide" : "Show"} Session List`}
                    shortcut={frame.shortcut("toggleSessionList")}
                    aria-expanded={listOpen}
                    onClick={() => dispatch(toggleSessionList())}
                  />
                ) : (
                  <IconButton
                    className="workspace-titlebar-compose"
                    icon="newSession"
                    label="New Session"
                    shortcut={composeShortcut}
                    tabIndex={sidebarOpen ? -1 : 0}
                    aria-hidden={sidebarOpen || undefined}
                    onClick={compose}
                  />
                )}
              </WorkspaceTitlebar>
              <Columns
                region={region}
                root={root}
                sidebarOpen={sidebarOpen}
                listOpen={listOpen}
                sidebarWidth={sidebarWidth}
                listWidth={listWidth}
                top={top}
              />
              <OverviewLayer root={root} />
              {switcher ? (
                <SwitcherHost
                  mode={switcher}
                  onClose={() => setSwitcher(null)}
                  onPick={pick}
                />
              ) : null}
            </div>
          </FlipScope>
        </SidebarPeekProvider>
      </ListedChannelProvider>
    </WorkspaceFrameProvider>
  )
}

/** The switcher, starting new sessions in the focused pane's channel. */
function SwitcherHost({
  mode,
  onClose,
  onPick,
}: {
  mode: SwitcherMode
  onClose: () => void
  onPick: (row: SwitcherRow, beside: boolean) => void
}) {
  const channelId = useWorkspaceSelector(selectFocusedChannel) ?? ""
  return (
    <QuickSwitcher mode={mode} channelId={channelId} onClose={onClose} onPick={onPick} />
  )
}

/**
 * The sidebar region as the layout composes it, its edges, and the chat
 * area. It renders again only when a column opens or closes; each part reads
 * what it shows for itself.
 */
const Columns = memo(function Columns({
  region,
  root,
  sidebarOpen,
  listOpen,
  sidebarWidth,
  listWidth,
  top,
}: {
  region: SidebarRegion
  root: RefObject<HTMLElement | null>
  sidebarOpen: boolean
  listOpen: boolean
  sidebarWidth: number
  listWidth: number
  top: ReturnType<typeof IconButton> | undefined
}) {
  const dispatch = useWorkspaceDispatch()
  const sidebarFrom = useRef<number | null>(0)
  const listFrom = useRef<number | null>(0)
  const limits = region.sidebar.limits
  // Dragged past its narrowest, a column folds away — the person's own choice —
  // and dragged back out from where it folded, it opens (`draggedEdge`).
  const dragSidebar = (delta: number) => {
    const { open, width } = draggedEdge(sidebarFrom.current, delta, limits)
    if (!open) dispatch(toggleSidebar({ open: false }))
    else dispatch(resizeSidebar({ width }))
  }
  const dragList = (delta: number) => {
    const { open, width } = draggedEdge(listFrom.current, delta, sessionListLimits)
    if (open !== listOpen) dispatch(toggleSessionList({ open }))
    if (open) dispatch(resizeSessionList({ width }))
  }
  return (
    <>
      <SourceList variant={region.sidebar.variant} top={top} />
      {sidebarOpen ? (
        <ResizeEdge
          label="Resize Sidebar"
          value={{ now: sidebarWidth, min: limits.min, max: limits.max }}
          onStart={() =>
            (sidebarFrom.current = drawnWidth(root.current, ".workspace-sidebar"))
          }
          onMove={dragSidebar}
        />
      ) : null}
      {region.sessionList ? <SessionList /> : null}
      {region.sessionList && (listOpen || sidebarOpen) ? (
        <ResizeEdge
          label="Resize Session List"
          className={
            listOpen ? "workspace-list-edge" : "workspace-list-edge workspace-edge-folded"
          }
          value={{
            now: listOpen ? listWidth : 0,
            min: sessionListLimits.min,
            max: sessionListLimits.max,
          }}
          onStart={() =>
            (listFrom.current = listOpen
              ? drawnWidth(root.current, ".workspace-list")
              : null)
          }
          onMove={dragList}
        />
      ) : null}
      <PaneGrid />
    </>
  )
})
