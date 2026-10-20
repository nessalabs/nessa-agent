/**
 * The workspace window's markup. It takes what `useWorkspaceWindow` decided
 * and does not read the store or dispatch. A view that shows one fact —
 * the content mark, the switcher — keeps that subscription itself.
 */
import { memo, useMemo, type CSSProperties, type ReactNode } from "react"
import { EdgePeekStrip } from "../../../ui/edge-peek-strip"
import { HistoryButtons } from "../../../ui/history-buttons"
import { ResizeEdge } from "../../../ui/resize-edge"
import { FlipScope, type SplitPanesSource } from "../../../split-panes"
import { EscapeScopesProvider } from "../../adapters/dom/widget-escape"
import { sessionListLimits, type ColumnLimits } from "../../model/window-fit"
import type { WindowArrangement } from "../../model/shown-window"
import { IconButton } from "../../../ui/icon-button"
import {
  SideRail,
  SideRailButton,
  RailView,
  holdsWindowControls,
} from "../chrome/side-rail"
import { WorkspaceTitlebar } from "../chrome/workspace-titlebar"
import { OverviewLayer } from "../overview/overview-layer"
import { PaneGrid } from "../panes/pane-grid"
import { SessionList } from "../session-list/session-list"
import { ShareDialogHost } from "../share/share-dialog"
import { DeleteSessionHost } from "../session-row-actions"
import { OverviewQuietProvider } from "../source-list/overview-quiet"
import { SourceList } from "../source-list/source-list"
import {
  ListedChannelProvider,
  SidebarPeekProvider,
  WorkspaceFrameProvider,
} from "../workspace-frame"
import type { WorkspaceWindowModel } from "./use-workspace-window"
import { ColumnFit, FoldMark, ListMeasureHold } from "./column-paint"
import { ContentMark } from "./content-mark"
import { SwitcherHost } from "./switcher-host"
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

export function WorkspaceLayout(model: WorkspaceWindowModel) {
  const {
    region,
    hostKind,
    browserSurface,
    theme,
    root,
    store,
    escapeScopes,
    frame,
    peek,
    shape,
    splitPanes,
    railOffered,
    railDrawn,
    railRoom,
    railWidth,
    railItem,
    shownRailItem,
    sidebarOpen,
    sidebarChosenOpen,
    listOpen,
    listedChannel,
    sidebarWidth,
    listWidth,
    windowWidth,
    switcher,
    compose,
    composeShortcut,
    onPickRail,
    onToggleRail,
    onToggleSidebar,
    onToggleSessionList,
    onRevealSidebar,
    onCloseSwitcher,
    onPickSwitcher,
    onSidebarDragStart,
    onSidebarDragMove,
    onListDragStart,
    onListDragMove,
  } = model
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
          <ListedChannelProvider value={listedChannel}>
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
                  data-sidebar-chosen={sidebarChosenOpen ? "open" : "closed"}
                  style={{ "--rail-width": `${railWidth}px` } as CSSProperties}
                >
                  {/* The light behind the window spans the rail too, so the
                      rail and the workspace stand on one ground. */}
                  <div className="desktop-ambient" aria-hidden="true">
                    <span className="desktop-grain" />
                  </div>
                  {railOffered ? (
                    <>
                      <SideRail open={railDrawn} active={railItem} onPick={onPickRail} />
                      <SideRailButton
                        open={railDrawn}
                        room={railRoom}
                        toggle={onToggleRail}
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
                      <EdgePeekStrip peek={peek} onDragOut={onRevealSidebar} />
                    )}
                    <WorkspaceTitlebar>
                      <IconButton
                        icon="sidebar"
                        label={`${sidebarOpen ? "Hide" : "Show"} Sidebar`}
                        shortcut={frame.shortcut("toggleSidebar")}
                        aria-expanded={sidebarOpen}
                        aria-controls="workspace-sidebar"
                        onClick={onToggleSidebar}
                      />
                      <HistoryButtons />
                      {region.sessionList ? (
                        <IconButton
                          icon="sessionList"
                          label={`${listOpen ? "Hide" : "Show"} Session List`}
                          shortcut={frame.shortcut("toggleSessionList")}
                          aria-expanded={listOpen}
                          onClick={onToggleSessionList}
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
                    <ColumnFit
                      windowWidth={windowWidth}
                      sidebarWidth={sidebarWidth}
                      listWidth={listWidth}
                    />
                    <ListMeasureHold root={root} />
                    <FoldMark root={root} />
                    <Columns
                      region={region}
                      sidebarOpen={sidebarOpen}
                      listOpen={listOpen}
                      sidebarWidth={sidebarWidth}
                      listWidth={listWidth}
                      top={top}
                      splitPanes={splitPanes}
                      onSidebarDragStart={onSidebarDragStart}
                      onSidebarDragMove={onSidebarDragMove}
                      onListDragStart={onListDragStart}
                      onListDragMove={onListDragMove}
                    />
                    <OverviewLayer root={root} />
                    <ShareDialogHost />
                    <DeleteSessionHost />
                    {switcher ? (
                      <SwitcherHost
                        mode={switcher}
                        onClose={onCloseSwitcher}
                        onPick={onPickSwitcher}
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

/**
 * The sidebar region as the layout composes it, its edges, and the chat
 * area. It renders again only when a column opens or closes; each part reads
 * what it shows for itself.
 */
const Columns = memo(function Columns({
  region,
  sidebarOpen,
  listOpen,
  sidebarWidth,
  listWidth,
  top,
  splitPanes,
  onSidebarDragStart,
  onSidebarDragMove,
  onListDragStart,
  onListDragMove,
}: {
  region: SidebarRegion
  sidebarOpen: boolean
  listOpen: boolean
  sidebarWidth: number
  listWidth: number
  top: ReactNode
  /** The panes' source, which the chat area's grid and the window's drag share. */
  splitPanes: SplitPanesSource
  onSidebarDragStart: () => void
  onSidebarDragMove: (delta: number) => void
  onListDragStart: () => void
  onListDragMove: (delta: number) => void
}) {
  const limits = region.sidebar.limits
  return (
    <>
      <SourceList variant={region.sidebar.variant} open={sidebarOpen} top={top} />
      {sidebarOpen ? (
        <ResizeEdge
          label="Resize Sidebar"
          value={{ now: sidebarWidth, min: limits.min, max: limits.max }}
          onStart={onSidebarDragStart}
          onMove={onSidebarDragMove}
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
            onStart={onListDragStart}
            onMove={onListDragMove}
          />
        ) : null}
        <PaneGrid source={splitPanes} />
      </div>
    </>
  )
})
