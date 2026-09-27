import { memo, useRef, type CSSProperties, type RefObject } from "react"
import type { HostKind } from "../../../../host/features"
import {
  fitToWindow,
  resizeSessionList,
  resizeSidebar,
  toggleSessionList,
  toggleSidebar,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChrome, selectView } from "../../adapters/store/selectors"
import { useFitOnResize, useWindowWidth } from "../../adapters/dom/window-width"
import { paneLimits } from "../../model/pane-layout"
import { columnWidth, sessionListLimits, sidebarLimits } from "../../model/window-fit"
import { IconButton } from "../chrome/icon-button"
import { ResizeEdge } from "../chrome/resize-edge"
import { WorkspaceTitlebar } from "../chrome/workspace-titlebar"
import { PaneGrid } from "../panes/pane-grid"
import { SessionList } from "../session-list/session-list"
import { SourceList } from "../source-list/source-list"
import { threeColumnsShortcuts } from "./shortcuts"
import {
  useLayoutFrame,
  useMotionShape,
  useWorkspaceKeys,
  WorkspaceShell,
} from "./workspace-shell"

/** The sidebar's width until the person drags it. */
const sidebarDefault = 240

/**
 * Three columns, as in Mail or Finder: the sidebar of channels, the chosen
 * channel's session list, and the chat panes. Either column folds away (⌘B,
 * ⌥⌘S), and both fold on their own when the window grows too narrow for the
 * panes beside them. This file only arranges the parts.
 */
export function ThreeColumns({
  hostKind,
  browserSurface,
}: {
  hostKind: HostKind
  browserSurface: boolean
}) {
  const dispatch = useWorkspaceDispatch()
  const root = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const chrome = useWorkspaceSelector(selectChrome)
  const view = useWorkspaceSelector(selectView)
  const windowWidth = useWindowWidth()
  const sidebarWidth = columnWidth(
    chrome.sidebarWidth ?? sidebarDefault,
    sidebarLimits,
    windowWidth,
  )
  const listWidth = columnWidth(chrome.sessionListWidth, sessionListLimits, windowWidth)
  useFitOnResize(windowWidth, (width) =>
    dispatch(
      fitToWindow({ windowWidth: width, sidebarWidth, sessionListWidth: listWidth }),
    ),
  )
  const frame = useLayoutFrame({
    root,
    bindings: threeColumnsShortcuts,
    sidebarSpare: chrome.sidebarOpen ? sidebarWidth + paneLimits.gutter : 0,
  })
  useWorkspaceKeys(threeColumnsShortcuts, frame, {
    toggleSessionList: () => {
      dispatch(toggleSessionList())
    },
    // Searching brings the list back first; it is inert while hidden.
    search: () => {
      dispatch(toggleSessionList({ open: true }))
      requestAnimationFrame(() => searchRef.current?.focus())
    },
  })
  const shape = useMotionShape(chrome.sidebarOpen, chrome.sessionListOpen)
  const sidebarLabel = `${chrome.sidebarOpen ? "Hide" : "Show"} Sidebar`
  const listLabel = `${chrome.sessionListOpen ? "Hide" : "Show"} Session List`
  return (
    <WorkspaceShell
      layout="columns"
      hostKind={hostKind}
      browserSurface={browserSurface}
      root={root}
      frame={frame}
      listedChannel={
        chrome.sessionListOpen && view.kind === "channel" ? view.channelId : null
      }
      shape={shape}
      data={{
        "data-sidebar": chrome.sidebarOpen ? "open" : "closed",
        "data-list": chrome.sessionListOpen ? "open" : "closed",
        "data-panes-alone": (!chrome.sidebarOpen && !chrome.sessionListOpen) || undefined,
      }}
      style={
        {
          "--workspace-sidebar-width": `${sidebarWidth}px`,
          "--workspace-list-width": `${listWidth}px`,
        } as CSSProperties
      }
    >
      <WorkspaceTitlebar>
        <IconButton
          icon="sidebar"
          label={sidebarLabel}
          shortcut={frame.shortcut("toggleSidebar")}
          aria-expanded={chrome.sidebarOpen}
          aria-controls="workspace-sidebar"
          onClick={() => dispatch(toggleSidebar())}
        />
        <IconButton
          icon="sessionList"
          label={listLabel}
          shortcut={frame.shortcut("toggleSessionList")}
          aria-expanded={chrome.sessionListOpen}
          onClick={() => dispatch(toggleSessionList())}
        />
      </WorkspaceTitlebar>
      <Columns
        sidebarOpen={chrome.sidebarOpen}
        listOpen={chrome.sessionListOpen}
        searchRef={searchRef}
      />
    </WorkspaceShell>
  )
}

/** A column's width as drawn, which a drag of its edge starts from. */
const drawnWidth = (selector: string) =>
  document.querySelector(selector)?.getBoundingClientRect().width ?? 0

/**
 * The three columns and the edges between them. It renders again only when a
 * column opens or closes; each column reads what it shows for itself.
 */
const Columns = memo(function Columns({
  sidebarOpen,
  listOpen,
  searchRef,
}: {
  sidebarOpen: boolean
  listOpen: boolean
  searchRef: RefObject<HTMLInputElement | null>
}) {
  const dispatch = useWorkspaceDispatch()
  const sidebarFrom = useRef(0)
  const listFrom = useRef(0)
  return (
    <>
      <SourceList variant="channels" />
      {sidebarOpen ? (
        <ResizeEdge
          label="Resize Sidebar"
          onStart={() => (sidebarFrom.current = drawnWidth(".workspace-sidebar"))}
          onMove={(delta) =>
            dispatch(resizeSidebar({ width: sidebarFrom.current + delta }))
          }
        />
      ) : null}
      <SessionList searchRef={searchRef} />
      {listOpen ? (
        <ResizeEdge
          label="Resize Session List"
          onStart={() => (listFrom.current = drawnWidth(".workspace-list"))}
          onMove={(delta) =>
            dispatch(resizeSessionList({ width: listFrom.current + delta }))
          }
        />
      ) : null}
      <PaneGrid />
    </>
  )
})
