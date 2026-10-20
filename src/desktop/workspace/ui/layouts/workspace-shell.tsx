/**
 * The workspace window, whichever layout it is in. Everything the two
 * layouts share lives here, once: the window's root and its light; the
 * titlebar's controls (sidebar toggle, Back, Forward); the keyboard
 * (`workspaceShortcuts`, one map); the quick switcher (⌘K, ⌘\); the pane grid
 * and everything in a pane; focus following the focused pane; drag and drop;
 * the folded sidebar's reveal from the edge; fitting the side columns to the
 * window; the Agents overview's layer over the list and the panes
 * (`ui/overview/`); Escape for the widget in front (`widget-escape.ts`). A layout (`three-columns.tsx`, `sessions-in-sidebar.tsx`)
 * says only how the sidebar region is composed — which sidebar, and whether a
 * session list stands beside it — as a `SidebarRegion`.
 */
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
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
import { useSideRailPreference } from "../../../adapters/window-preferences"
import { draggedEdge } from "../../../model/side-column"
import { EdgePeekStrip } from "../../../ui/edge-peek-strip"
import { HistoryButtons } from "../../../ui/history-buttons"
import { ResizeEdge } from "../../../ui/resize-edge"
import { workspaceDragOptions } from "../../adapters/dom/split-panes-drag"
import { FlipScope, useSplitPanesDrag, type SplitPanesSource } from "../../../split-panes"
import { useFocusFollowsPane } from "../../adapters/dom/focus"
import {
  EscapeScopesProvider,
  useWidgetEscape,
  type EscapeScopes,
} from "../../adapters/dom/widget-escape"
import type { WindowArrangement } from "../../model/shown-window"
import { labelOf, useKeyBindings, type Binding } from "../../adapters/dom/shortcuts"
import { useFitOnResize } from "../../adapters/dom/window-width"
import {
  arrangementShown,
  canOpenBeside,
  closeInFront,
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
import { reviewToOpen, reviewWaitsIn } from "../../adapters/gateway/review-notices"
import {
  selectChrome,
  selectDraft,
  selectSession,
  selectColumnCount,
  selectFocusedChannel,
  selectFocusedPaneKey,
  selectContentKind,
  selectPanes,
  selectSessionListOpen,
  selectSidebarOpen,
  selectView,
} from "../../adapters/store/selectors"
import { workspaceSplitPanes } from "../../adapters/store/split-panes-source"
import {
  layoutShape,
  panesOf,
  type Direction,
} from "../../../split-panes/model/pane-layout"
import type { SwitcherRow } from "../../model/session-search"
import {
  columnWidth,
  railFits,
  sessionListLimits,
  type ColumnLimits,
} from "../../model/window-fit"
import { IconButton } from "../../../ui/icon-button"
import {
  SideRail,
  SideRailButton,
  RailView,
  railItems,
  railWidth,
  holdsWindowControls,
} from "../chrome/side-rail"
import { WorkspaceTitlebar } from "../chrome/workspace-titlebar"
import { OverviewLayer } from "../overview/overview-layer"
import { PaneGrid } from "../panes/pane-grid"
import { QuickSwitcher, type SwitcherMode } from "../quick-switcher/quick-switcher"
import { SessionList } from "../session-list/session-list"
import { ShareDialogHost } from "../share/share-dialog"
import { DeleteSessionHost } from "../session-row-actions"
import { OverviewQuietProvider } from "../source-list/overview-quiet"
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
  /** Which layout this is, as the window tells the person's agents. */
  readonly arrangement: WindowArrangement
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
 * handler may decline an unavailable action; its bound key remains consumed.
 * Where the caret goes after
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
function useMotionShape(...columns: boolean[]): string {
  const panes = useWorkspaceSelector((state) => {
    const layout = selectPanes(state)
    return layout ? layoutShape(layout) : ""
  })
  return `${panes}|${columns.join(",")}`
}

/** Sets the workspace's `data-content` from its own subscription (`layouts.test.tsx`). */
function ContentMark({ root }: { root: RefObject<HTMLElement | null> }) {
  const contentShown = useWorkspaceSelector(selectContentKind)
  useLayoutEffect(() => {
    root.current?.setAttribute("data-content", contentShown)
  }, [contentShown, root])
  return null
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
  useEffect(() => {
    // A notice can name a conversation the list has not shown yet: the
    // gateway sends a waiting review before the list frame that names it,
    // and a session the workspace does not hold cannot be opened. The one
    // clicked last is opened as soon as the window holds it, while a review
    // there still waits: once none does, the click has nothing left to show,
    // and a conversation that turns up later does not take a pane.
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
  // The window tells the person's agents what it shows only while a layout
  // of panes is on screen: the classic shell draws none.
  useEffect(() => {
    dispatch(arrangementShown({ arrangement: region.arrangement }))
    return () => {
      dispatch(arrangementShown({ arrangement: null }))
    }
  }, [dispatch, region.arrangement])
  const root = useRef<HTMLDivElement>(null)
  const [theme] = useThemePreference()
  const chrome = useWorkspaceSelector(selectChrome)
  const sidebarOpen = useWorkspaceSelector(selectSidebarOpen)
  const listDrawn = useWorkspaceSelector(selectSessionListOpen)
  const listOpen = region.sessionList && listDrawn
  const view = useWorkspaceSelector(selectView)
  const columns = useWorkspaceSelector(selectColumnCount)
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
  const rail = railWidth(hostKind)
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
  // The rail stands beside the workspace, not in it: the columns fit what is left.
  const windowWidth = fullWidth - (railDrawn ? rail : 0)
  useFitOnResize(windowWidth, columns, (width) =>
    dispatch(
      fitToWindow({ windowWidth: width, sidebarWidth, sessionListWidth: listWidth }),
    ),
  )

  const [switcher, setSwitcher] = useState<SwitcherMode | null>(null)
  const frame = useWindowFrame(root, workspaceShortcuts, setSwitcher)
  // While the switcher is up it has the keyboard, but for ⌘K, which closes it.
  // Under an item's full view the workspace answers no keys at all.
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
  )
  useFocusFollowsPane(store, root)
  // The panes' drag spans the window: sessions are picked up from its lists.
  const splitPanes = useMemo(() => workspaceSplitPanes(store), [store])
  const dragOptions = useMemo(workspaceDragOptions, [])
  useSplitPanesDrag(root, splitPanes, dragOptions)
  const peek = useEdgePeek(!sidebarOpen, sidebarOpen)
  // Each widget host on the page, by scope, for Escape to find (`widget-escape.ts`).
  const [escapeScopes] = useState<EscapeScopes>(() => new Map())
  useWidgetEscape({ store, root, scopes: escapeScopes })
  const shape = useMotionShape(sidebarOpen, listOpen, railDrawn)

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
    <OverviewQuietProvider store={store} root={root}>
      <WorkspaceFrameProvider value={frame}>
        <EscapeScopesProvider value={escapeScopes}>
          <ListedChannelProvider value={listOpen ? view.channelId : null}>
            <SidebarPeekProvider value={peek}>
              <FlipScope shape={shape} root={root}>
                <div
                  className="workspace-window"
                  data-rail={!railOffered ? "off" : railDrawn ? "open" : "closed"}
                  data-rail-view={shownRailItem?.id}
                  data-rail-holds={
                    holdsWindowControls(hostKind) ? "window-controls" : undefined
                  }
                  data-host={hostKind}
                  data-surface={browserSurface ? "browser" : "window"}
                  data-desktop-theme={theme}
                  data-sidebar={sidebarOpen ? "open" : "closed"}
                  // The person's own choice, apart from a fold for room: the
                  // rail's toggle hides only with a sidebar they closed.
                  data-sidebar-chosen={chrome.sidebar.open ? "open" : "closed"}
                  style={{ "--rail-width": `${rail}px` } as CSSProperties}
                >
                  {/* The light behind the window spans the rail too, so the
                      rail and the workspace stand on one ground. */}
                  <div className="desktop-ambient" aria-hidden="true">
                    <span className="desktop-grain" />
                  </div>
                  {railOffered ? (
                    <>
                      <SideRail
                        open={railDrawn}
                        active={railItem}
                        onPick={(id) => {
                          // Picking a place is leaving whatever had the keyboard here.
                          setSwitcher(null)
                          setRailItem(id)
                        }}
                      />
                      <SideRailButton
                        open={railDrawn}
                        room={railRoom}
                        toggle={() => setRailOpen((open) => !open)}
                      />
                    </>
                  ) : null}
                  <div
                    ref={root}
                    className="workspace"
                    // Under an item's full view it takes no focus, pointer or Escape.
                    inert={shownRailItem !== undefined || undefined}
                    data-workspace
                    data-host={hostKind}
                    data-surface={browserSurface ? "browser" : "window"}
                    data-desktop-theme={theme}
                    data-peek={(peek.shown && !peek.handedOff) || undefined}
                    data-sidebar={sidebarOpen ? "open" : "closed"}
                    data-list={
                      region.sessionList ? (listOpen ? "open" : "closed") : undefined
                    }
                    data-panes-alone={(!sidebarOpen && !listOpen) || undefined}
                    style={
                      {
                        "--workspace-sidebar-width": `${sidebarWidth}px`,
                        ...(region.sessionList
                          ? { "--workspace-list-width": `${listWidth}px` }
                          : {}),
                      } as CSSProperties
                    }
                  >
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
                      <HistoryButtons />
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
                    <ContentMark root={root} />
                    <Columns
                      region={region}
                      root={root}
                      sidebarOpen={sidebarOpen}
                      listOpen={listOpen}
                      sidebarWidth={sidebarWidth}
                      listWidth={listWidth}
                      top={top}
                      splitPanes={splitPanes}
                    />
                    <OverviewLayer root={root} />
                    <ShareDialogHost />
                    <DeleteSessionHost />
                    {switcher ? (
                      <SwitcherHost
                        mode={switcher}
                        onClose={() => setSwitcher(null)}
                        onPick={pick}
                      />
                    ) : null}
                  </div>
                  {shownRailItem ? <RailView item={shownRailItem} /> : null}
                </div>
              </FlipScope>
            </SidebarPeekProvider>
          </ListedChannelProvider>
        </EscapeScopesProvider>
      </WorkspaceFrameProvider>
    </OverviewQuietProvider>
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
  splitPanes,
}: {
  region: SidebarRegion
  root: RefObject<HTMLElement | null>
  sidebarOpen: boolean
  listOpen: boolean
  sidebarWidth: number
  listWidth: number
  top: ReturnType<typeof IconButton> | undefined
  /** The panes' source, which the chat area's grid and the window's drag share. */
  splitPanes: SplitPanesSource
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
      {/* What the Agents overview covers, and makes inert (`overview-layer.tsx`). */}
      <div className="workspace-content">
        {region.sessionList ? <SessionList /> : null}
        {region.sessionList && (listOpen || sidebarOpen) ? (
          <ResizeEdge
            label="Resize Session List"
            className={
              listOpen
                ? "workspace-list-edge"
                : "workspace-list-edge workspace-edge-folded"
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
        <PaneGrid source={splitPanes} />
      </div>
    </>
  )
})
