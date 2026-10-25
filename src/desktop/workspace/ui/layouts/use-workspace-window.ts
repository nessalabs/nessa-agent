/**
 * The workspace window's orchestration: reviews, the arrangement the agents
 * are told, the rail and the side columns, keys, the switcher, focus, drag,
 * and Escape. `WorkspaceShell` calls this and passes the result to
 * `WorkspaceLayout`, which renders it. See
 * `docs/design/workspace-shell-split.md`.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react"
import type { HostKind } from "../../../../host/features"
import { reducedMotion } from "../../../adapters/motion-preference"
import { useThemePreference } from "../../../adapters/theme-preference"
import { useEdgePeek, type EdgePeekControls } from "../../../adapters/use-edge-peek"
import { useWindowWidth } from "../../../adapters/window-width"
import {
  usePanelPreference,
  useSideRailPreference,
} from "../../../adapters/window-preferences"
import {
  clampPanel,
  PANEL_MAX_WIDTH,
  PANEL_MIN_WIDTH,
  panelWidth,
} from "../../../model/right-panel"
import { draggedEdge } from "../../../model/side-column"
import type { DesktopThemeId } from "../../../model/theme"
import type { DesktopStore } from "../../../store"
import { workspaceDragOptions } from "../../adapters/dom/split-panes-drag"
import { useSplitPanesDrag, type SplitPanesSource } from "../../../split-panes"
import { useFocusFollowsPane } from "../../adapters/dom/focus"
import { useWidgetEscape, type EscapeScopes } from "../../adapters/dom/widget-escape"
import { labelOf, useKeyBindings, type Binding } from "../../adapters/dom/shortcuts"
import { drawn } from "../../../model/side-column"
import {
  arrangementShown,
  canOpenBeside,
  closeInFront,
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
import { reviewToOpen, reviewWaitsIn } from "../../adapters/gateway/review-notices"
import { paneBoxFrame, usePaintedWithPanes } from "../../adapters/store/paint-with-panes"
import {
  selectChrome,
  selectColumnCount,
  selectDraft,
  selectFocusedPaneKey,
  selectActiveTab,
  selectInFront,
  selectPaneShape,
  selectPanes,
  selectSession,
  selectSessionListOpen,
  selectView,
} from "../../adapters/store/selectors"
import { workspaceSplitPanes } from "../../adapters/store/split-panes-source"
import { panesOf, type Direction } from "../../../split-panes/model/pane-layout"
import { sameContent } from "../../application/workspace-state"
import type { SwitcherRow } from "../../model/session-search"
import { columnWidth, railFits, sessionListLimits } from "../../model/window-fit"
import { railItems, SIDE_RAIL_WIDTH, type RailItem } from "../chrome/side-rail"
import type { SwitcherMode } from "../quick-switcher/quick-switcher"
import { type ShortcutCommand, type WorkspaceFrame } from "../workspace-frame"
import { workspaceShortcuts } from "./shortcuts"
import type { SidebarRegion } from "./workspace-layout"

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
 * handler may decline an unavailable action; its bound key remains consumed.
 * Where the caret goes after
 * each is not the keys' business: it follows the focused pane
 * (`adapters/dom/focus.ts`).
 */
function useWorkspaceKeys(
  own: Partial<Record<ShortcutCommand, () => boolean | void>>,
  /** While true, only `own` commands run: something modal has the keyboard. */
  paused: boolean,
  /** Runs first for every command the keys bring. */
  before?: (command: ShortcutCommand) => void,
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
    before?.(command)
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
        // What is in front: a widget over the panes, else the focused pane.
        dispatch(closeInFront())
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
function useMotionShape(...columns: boolean[]): { shape: string; columns: string } {
  // Painted with the new boxes, so the click does not render the window for a shape it has not drawn.
  const panes = usePaintedWithPanes(selectPaneShape, paneBoxFrame)
  return { shape: panes, columns: columns.join(",") }
}

/** A column's width as laid out — never as a slide draws it — which a drag of its edge starts from. */
const drawnWidth = (root: HTMLElement | null, selector: string) =>
  root?.querySelector<HTMLElement>(selector)?.offsetWidth ?? 0

/**
 * A notice can name a conversation the list has not shown yet: the gateway
 * sends a waiting review before the list frame that names it, and a session
 * the workspace does not hold cannot be opened. The one clicked last is
 * opened as soon as the window holds it, while a review there still waits:
 * once none does, the click has nothing left to show, and a conversation
 * that turns up later does not take a pane.
 */
function useReviewOpen() {
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
  useEffect(() => {
    let asked: string | null = null
    const openAsked = () => {
      if (asked === null) return
      if (!reviewWaitsIn(asked)) {
        asked = null
        return
      }
      const state = store.getState()
      if (!selectSession(state, asked) && !selectDraft(state, asked)) return
      const sessionId = asked
      asked = null
      void dispatch(openSession({ sessionId }))
    }
    const open = (event: Event) => {
      asked = reviewToOpen(event) ?? asked
      openAsked()
    }
    const unsubscribe = store.subscribe(openAsked)
    window.addEventListener("nessa-open-review", open)
    return () => {
      unsubscribe()
      window.removeEventListener("nessa-open-review", open)
    }
  }, [dispatch, store])
}

/** The window tells the person's agents what it shows only while a layout of panes is on screen. */
function useShownArrangement(region: SidebarRegion) {
  const dispatch = useWorkspaceDispatch()
  useEffect(() => {
    dispatch(arrangementShown({ arrangement: region.arrangement }))
    return () => {
      dispatch(arrangementShown({ arrangement: null }))
    }
  }, [dispatch, region.arrangement])
}

function useWorkspaceChrome(region: SidebarRegion) {
  // Painted with the new boxes. The fold's attribute is set a frame earlier
  // (`FoldMark`), while the spare hold is on. A person's toggle still paints
  // in the turn: it does not change the column count.
  const chrome = usePaintedWithPanes(selectChrome, paneBoxFrame)
  const columns = usePaintedWithPanes(selectColumnCount, paneBoxFrame)
  const sidebarOpen = drawn(chrome.sidebar)
  const listDrawn = useWorkspaceSelector(selectSessionListOpen)
  const listOpen = region.sessionList && listDrawn
  const view = useWorkspaceSelector(selectView)
  const [railOpen, setRailOpen] = useState(true)
  const [railItem, setRailItem] = useState("agents")
  // A preview, off until turned on in Settings › Advanced › Experimental.
  const [railFlag] = useSideRailPreference()
  const railOffered = railFlag === "on"
  // Turned off, it forgets the place it had open: turned on again, it opens on Agents.
  if (!railOffered && railItem !== "agents") setRailItem("agents")
  // An item other than Agents has the window: the workspace under it is inert.
  const shownRailItem = railOffered
    ? railItems.find((each) => each.id === railItem && each.id !== "agents")
    : undefined
  const fullWidth = useWindowWidth()
  const sidebarWidth = columnWidth(
    chrome.sidebarWidth ?? region.sidebar.defaultWidth,
    region.sidebar.limits,
    fullWidth,
  )
  const listWidth = region.sessionList
    ? columnWidth(chrome.sessionListWidth, sessionListLimits, fullWidth)
    : 0
  // One width for fitting and drawing the compact rail below the top bar.
  const rail = SIDE_RAIL_WIDTH
  // The rail is the first column to give way for room (`railFits`): it is drawn
  // only where it folds nothing the person has open. A full view has no columns.
  const railRoom =
    shownRailItem !== undefined ||
    railFits(
      {
        sidebarOpen: chrome.sidebar.open,
        sessionListOpen: region.sessionList && chrome.sessionList.open,
        sidebarWidth,
        sessionListWidth: listWidth,
      },
      fullWidth,
      columns,
      rail,
    )
  const railDrawn = railOffered && railOpen && railRoom
  // The right panel, beside the panes. Maximized, it covers the
  // workspace until Escape or its button; a full view hides it.
  const [panelPlace, setPanelPlace] = usePanelPreference()
  const [panelMaximized, setPanelMaximized] = useState(false)
  const panelOpen = panelPlace.open && shownRailItem === undefined
  if (!panelOpen && panelMaximized) setPanelMaximized(false)
  usePanelHeldByChat(panelMaximized, setPanelMaximized)
  const panelDrawnWidth = panelWidth(panelPlace.width, fullWidth)
  // The rail and the panel stand beside the workspace's columns, not among
  // them: the columns fit what is left. The fit itself is `ColumnFit`, so
  // this count does not render the window.
  const windowWidth =
    fullWidth - (railDrawn ? rail : 0) - (panelOpen ? panelDrawnWidth : 0)
  const toggleRail = useCallback(() => setRailOpen((open) => !open), [])
  const panelRef = useRef(panelPlace)
  panelRef.current = panelPlace
  const togglePanel = useCallback(() => {
    setPanelMaximized(false)
    setPanelPlace({ ...panelRef.current, open: !panelRef.current.open })
  }, [setPanelPlace])
  const resizePanel = useCallback(
    (width: number) => {
      const next = clampPanel(width)
      if (next !== panelRef.current.width)
        setPanelPlace({ ...panelRef.current, width: next })
    },
    [setPanelPlace],
  )
  const closePanel = useCallback(
    () => setPanelPlace({ ...panelRef.current, open: false }),
    [setPanelPlace],
  )
  const toggleMaximized = useCallback(() => setPanelMaximized((value) => !value), [])
  const dockPanel = useCallback(() => setPanelMaximized(false), [])
  return {
    sidebarOpen,
    listOpen,
    listedChannel: listOpen ? view.channelId : null,
    sidebarWidth,
    listWidth,
    windowWidth,
    railOffered,
    railDrawn,
    railRoom,
    railWidth: rail,
    railItem,
    shownRailItem,
    setRailItem,
    toggleRail,
    panelOpen,
    panelMaximized,
    panelWidth: panelDrawnWidth,
    setPanelMaximized,
    togglePanel,
    resizePanel,
    closePanel,
    toggleMaximized,
    dockPanel,
  }
}

/**
 * The right panel belongs to the chat in front, so expanded it stands for
 * that chat alone. It holds only while that chat stays in front: anything
 * that brings another — a new tab, a session opened, a tab switched to, a
 * pane split off, the overview or a widget over the panes — docks it again,
 * so nothing ever opens out of sight under it.
 */
function usePanelHeldByChat(maximized: boolean, setMaximized: (value: boolean) => void) {
  const store = useWorkspaceStore()
  useEffect(() => {
    if (!maximized) return
    const held = store.getState()
    return store.subscribe(() => {
      const now = store.getState()
      if (
        selectInFront(now) !== selectInFront(held) ||
        !sameContent(now.workspace.content, held.workspace.content)
      )
        setMaximized(false)
    })
  }, [store, maximized, setMaximized])
}

function useColumnDrag(
  region: SidebarRegion,
  root: RefObject<HTMLDivElement | null>,
  listOpen: boolean,
  /** The sidebar's drawn width; its box is wider by the gutter it stands in. */
  sidebarWidth: number,
) {
  const dispatch = useWorkspaceDispatch()
  const sidebarFrom = useRef<number | null>(0)
  const listFrom = useRef<number | null>(0)
  const sidebarNow = useRef(sidebarWidth)
  sidebarNow.current = sidebarWidth
  const limits = region.sidebar.limits
  const onSidebarDragStart = useCallback(() => {
    sidebarFrom.current = sidebarNow.current
  }, [])
  const onSidebarDragMove = useCallback(
    (delta: number) => {
      const { open, width } = draggedEdge(sidebarFrom.current, delta, limits)
      if (!open) dispatch(toggleSidebar({ open: false }))
      else dispatch(resizeSidebar({ width }))
    },
    [dispatch, limits],
  )
  const onListDragStart = useCallback(() => {
    listFrom.current = listOpen ? drawnWidth(root.current, ".workspace-list") : null
  }, [listOpen, root])
  const onListDragMove = useCallback(
    (delta: number) => {
      const { open, width } = draggedEdge(listFrom.current, delta, sessionListLimits)
      if (open !== listOpen) dispatch(toggleSessionList({ open }))
      if (open) dispatch(resizeSessionList({ width }))
    },
    [dispatch, listOpen],
  )
  return { onSidebarDragStart, onSidebarDragMove, onListDragStart, onListDragMove }
}

function useWorkspaceSwitcher(
  region: SidebarRegion,
  root: RefObject<HTMLDivElement | null>,
  shownRailItem: RailItem | undefined,
  togglePanel: () => void,
  /** Docks the expanded right panel, which a window command brings the workspace back from. */
  dockPanel: () => void,
) {
  const dispatch = useWorkspaceDispatch()
  const [switcher, setSwitcher] = useState<SwitcherMode | null>(null)
  const frame = useWindowFrame(root, workspaceShortcuts, setSwitcher)
  // While the switcher is up it has the keyboard, but for ⌘K, which closes it.
  // Under an item's full view the workspace answers no keys at all. Any window
  // key docks the expanded panel first, one it declines included: a key is
  // the person turning back to the workspace, and what it opens is seen.
  useWorkspaceKeys(
    shownRailItem !== undefined
      ? {}
      : {
          switcher: () => setSwitcher((open) => (open ? null : "open")),
          // Offered as "beside" only where a pane could open there; else it is a jump.
          openBeside: () =>
            switcher ? false : setSwitcher(dispatch(canOpenBeside()) ? "split" : "open"),
          toggleSessionList: () => {
            if (!region.sessionList) return false
            dispatch(toggleSessionList())
          },
          togglePanel: () => {
            if (switcher) return false
            togglePanel()
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
    switcher !== null || shownRailItem !== undefined,
    shownRailItem !== undefined ? undefined : dockPanel,
  )
  // Beside where the room allows it, in the focused pane's place where not —
  // what ⌘-click does — so a pick never goes nowhere, and a typed message is
  // never lost.
  const onPickSwitcher = (row: SwitcherRow, asked: boolean) => {
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
  return { switcher, setSwitcher, frame, onPickSwitcher }
}

export interface WorkspaceWindowModel {
  region: SidebarRegion
  hostKind: HostKind
  browserSurface: boolean
  theme: DesktopThemeId
  root: RefObject<HTMLDivElement | null>
  store: DesktopStore
  escapeScopes: EscapeScopes
  frame: WorkspaceFrame
  peek: EdgePeekControls
  shape: string
  /** Which side columns are open: a change of these alone is a drawer (`FlipScope`). */
  openColumns: string
  /** The window tab shown: its change is a cut, not a flight (`FlipScope`). */
  tab: string
  splitPanes: SplitPanesSource
  railOffered: boolean
  railDrawn: boolean
  railRoom: boolean
  /** Pixels the open rail takes, the same width the columns were fitted less of. */
  railWidth: number
  railItem: string
  shownRailItem: RailItem | undefined
  sidebarOpen: boolean
  listOpen: boolean
  listedChannel: string | null
  sidebarWidth: number
  listWidth: number
  /** The width the side columns are fitted to: the window, less an open rail. */
  windowWidth: number
  switcher: SwitcherMode | null
  onPickRail: (id: string) => void
  onToggleRail: () => void
  onToggleSidebar: () => void
  onToggleSessionList: () => void
  onRevealSidebar: () => void
  onCloseSwitcher: () => void
  onPickSwitcher: (row: SwitcherRow, beside: boolean) => void
  onSidebarDragStart: () => void
  onSidebarDragMove: (delta: number) => void
  onListDragStart: () => void
  onListDragMove: (delta: number) => void
  /** The right panel: open, covering the workspace, and its drawn width. */
  panelOpen: boolean
  panelMaximized: boolean
  panelWidth: number
  onTogglePanel: () => void
  onTogglePanelMaximized: () => void
  onPanelDragStart: () => void
  onPanelDragMove: (delta: number) => void
}

export function useWorkspaceWindow({
  region,
  hostKind,
  browserSurface,
}: {
  region: SidebarRegion
  hostKind: HostKind
  browserSurface: boolean
}): WorkspaceWindowModel {
  useReviewOpen()
  useShownArrangement(region)
  const root = useRef<HTMLDivElement>(null)
  const [theme] = useThemePreference()
  const store = useWorkspaceStore()
  const chrome = useWorkspaceChrome(region)
  const dragging = useColumnDrag(region, root, chrome.listOpen, chrome.sidebarWidth)
  const switching = useWorkspaceSwitcher(
    region,
    root,
    chrome.shownRailItem,
    chrome.togglePanel,
    chrome.dockPanel,
  )
  usePanelEscape(root, chrome.panelMaximized, chrome.setPanelMaximized)
  const panelDrag = usePanelDrag(chrome.panelWidth, chrome.resizePanel, chrome.closePanel)
  useFocusFollowsPane(store, root)
  // The panes' drag spans the window: sessions are picked up from its lists.
  const splitPanes = useMemo(() => workspaceSplitPanes(store), [store])
  const dragOptions = useMemo(workspaceDragOptions, [])
  useSplitPanesDrag(root, splitPanes, dragOptions)
  const peek = useEdgePeek(!chrome.sidebarOpen, chrome.sidebarOpen)
  // Each widget host on the page, by scope, for Escape to find (`widget-escape.ts`).
  const [escapeScopes] = useState<EscapeScopes>(() => new Map())
  useWidgetEscape({ store, root, scopes: escapeScopes })
  const { shape, columns: openColumns } = useMotionShape(
    chrome.sidebarOpen,
    chrome.listOpen,
    chrome.railDrawn,
    chrome.panelOpen,
  )
  // Painted with the boxes, as the shape is: another tab's layout is a cut.
  const tab = usePaintedWithPanes(selectActiveTab, paneBoxFrame)
  const dispatch = useWorkspaceDispatch()
  const onToggleSidebar = useCallback(() => dispatch(toggleSidebar()), [dispatch])
  const onToggleSessionList = useCallback(() => dispatch(toggleSessionList()), [dispatch])
  const onRevealSidebar = useCallback(
    () => dispatch(toggleSidebar({ open: true })),
    [dispatch],
  )
  const { setSwitcher } = switching
  const { setRailItem } = chrome
  const onCloseSwitcher = useCallback(() => setSwitcher(null), [setSwitcher])
  const onPickRail = useCallback(
    (id: string) => {
      // Picking a place is leaving whatever had the keyboard here.
      setSwitcher(null)
      setRailItem(id)
    },
    [setRailItem, setSwitcher],
  )
  return {
    region,
    hostKind,
    browserSurface,
    theme,
    root,
    store,
    escapeScopes,
    frame: switching.frame,
    peek,
    shape,
    openColumns,
    tab,
    splitPanes,
    railOffered: chrome.railOffered,
    railDrawn: chrome.railDrawn,
    railRoom: chrome.railRoom,
    railWidth: chrome.railWidth,
    railItem: chrome.railItem,
    shownRailItem: chrome.shownRailItem,
    sidebarOpen: chrome.sidebarOpen,
    listOpen: chrome.listOpen,
    listedChannel: chrome.listedChannel,
    sidebarWidth: chrome.sidebarWidth,
    listWidth: chrome.listWidth,
    windowWidth: chrome.windowWidth,
    switcher: switching.switcher,
    onPickRail,
    onToggleRail: chrome.toggleRail,
    onToggleSidebar,
    onToggleSessionList,
    onRevealSidebar,
    onCloseSwitcher,
    onPickSwitcher: switching.onPickSwitcher,
    panelOpen: chrome.panelOpen,
    panelMaximized: chrome.panelMaximized,
    panelWidth: chrome.panelWidth,
    onTogglePanel: chrome.togglePanel,
    onTogglePanelMaximized: chrome.toggleMaximized,
    ...panelDrag,
    ...dragging,
  }
}

/**
 * Escape leaves the maximized panel — what covers the workspace, so nothing
 * under it answers — unless a dialog, menu or list over it takes Escape
 * first, or the window itself is under Settings, whose keys are its own.
 */
function usePanelEscape(
  root: RefObject<HTMLElement | null>,
  maximized: boolean,
  setMaximized: (value: boolean) => void,
) {
  useEffect(() => {
    if (!maximized) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return
      if (root.current?.closest("[inert]")) return
      if (
        event.target instanceof Element &&
        event.target.closest('[role="dialog"], [role="menu"], [role="listbox"]')
      )
        return
      // Ahead of the workspace's own Escapes, which leave a prevented one alone.
      event.preventDefault()
      setMaximized(false)
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [root, maximized, setMaximized])
}

/** The panel's edge, on its left: dragged left it widens, and far enough right it folds. */
function usePanelDrag(
  /** The panel's drawn width; its box is wider by the gutter it stands in. */
  width: number,
  resize: (width: number) => void,
  close: () => void,
) {
  const from = useRef(0)
  const now = useRef(width)
  now.current = width
  const onPanelDragStart = useCallback(() => {
    from.current = now.current
  }, [])
  const onPanelDragMove = useCallback(
    (delta: number) => {
      const { open, width } = draggedEdge(from.current, -delta, {
        min: PANEL_MIN_WIDTH,
        max: PANEL_MAX_WIDTH,
      })
      if (!open) close()
      else resize(width)
    },
    [close, resize],
  )
  return { onPanelDragStart, onPanelDragMove }
}
