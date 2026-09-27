import {
  memo,
  useCallback,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react"
import type { HostKind } from "../../../../host/features"
import {
  fitToWindow,
  newSession,
  openBeside,
  openChannel,
  openSession,
  resizeSidebar,
  sendMessage,
  toggleSidebar,
} from "../../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import {
  selectFocusedChannel,
  selectFocusedPaneKey,
  selectSidebarOpen,
} from "../../adapters/store/selectors"
import { useFitOnResize, useWindowWidth } from "../../adapters/dom/window-width"
import { paneLimits } from "../../model/pane-layout"
import type { SwitcherRow } from "../../model/session-search"
import { columnWidth, treeSidebarLimits } from "../../model/window-fit"
import { IconButton } from "../chrome/icon-button"
import { ResizeEdge } from "../chrome/resize-edge"
import { WorkspaceTitlebar } from "../chrome/workspace-titlebar"
import { PaneGrid } from "../panes/pane-grid"
import { QuickSwitcher, type SwitcherMode } from "../quick-switcher/quick-switcher"
import { SourceList } from "../source-list/source-list"
import { sessionsInSidebarShortcuts } from "./shortcuts"
import {
  useLayoutFrame,
  useMotionShape,
  useWorkspaceKeys,
  WorkspaceShell,
} from "./workspace-shell"
import "./layouts.css"

/** The sidebar's width until the person drags it: room for a session's title and time. */
const sidebarDefault = 256

/**
 * Sessions in the sidebar: one source list, where each channel discloses its
 * sessions inline, beside the chat panes — no third column. ⌘K jumps to any
 * session or channel, and ⌘\ picks one to open beside. This file only
 * arranges the parts.
 */
export function SessionsInSidebar({
  hostKind,
  browserSurface,
}: {
  hostKind: HostKind
  browserSurface: boolean
}) {
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
  const root = useRef<HTMLDivElement>(null)
  const sidebarOpen = useWorkspaceSelector(selectSidebarOpen)
  const asked = useWorkspaceSelector((state) => state.workspace.chrome.sidebarWidth)
  const [switcher, setSwitcher] = useState<SwitcherMode | null>(null)
  const windowWidth = useWindowWidth()
  const sidebarWidth = columnWidth(
    asked ?? sidebarDefault,
    treeSidebarLimits,
    windowWidth,
  )
  useFitOnResize(windowWidth, (width) =>
    dispatch(fitToWindow({ windowWidth: width, sidebarWidth, sessionListWidth: 0 })),
  )
  const frame = useLayoutFrame({
    root,
    bindings: sessionsInSidebarShortcuts,
    sidebarSpare: sidebarOpen ? sidebarWidth + paneLimits.gutter : 0,
    openSwitcher: setSwitcher,
  })
  // While the switcher is up it has the keyboard, but for ⌘K, which closes it.
  useWorkspaceKeys(
    sessionsInSidebarShortcuts,
    frame,
    {
      switcher: () => setSwitcher((open) => (open ? null : "open")),
      openBeside: () => (switcher ? false : setSwitcher("split")),
    },
    switcher !== null,
  )
  const shape = useMotionShape(sidebarOpen)

  const pick = (row: SwitcherRow, beside: boolean) => {
    setSwitcher(null)
    const focused = selectFocusedPaneKey(store.getState())
    const room = beside && focused !== null ? frame.roomOf(focused) : undefined
    if (row.kind === "session") {
      dispatch(
        beside
          ? openBeside({ sessionId: row.session.id, room })
          : openSession({ sessionId: row.session.id }),
      )
    } else if (row.kind === "channel") {
      dispatch(openChannel({ channelId: row.channel.id, beside, room }))
    } else {
      const draftId = dispatch(
        newSession({
          channelId: row.channelId,
          beside: beside ? "right" : undefined,
          room,
        }),
      )
      if (draftId && row.text)
        void dispatch(
          sendMessage({ sessionId: draftId, text: row.text, initiator: "person" }),
        )
    }
  }

  const compose = useCallback(() => dispatch(newSession()), [dispatch])
  const composeShortcut = frame.shortcut("newSession")
  const top = useMemo(
    () => (
      <IconButton
        icon="newSession"
        label="New Session"
        shortcut={composeShortcut}
        onClick={compose}
      />
    ),
    [compose, composeShortcut],
  )
  const sidebarLabel = `${sidebarOpen ? "Hide" : "Show"} Sidebar`
  return (
    <WorkspaceShell
      layout="sidebar"
      hostKind={hostKind}
      browserSurface={browserSurface}
      root={root}
      frame={frame}
      listedChannel={null}
      shape={shape}
      data={{
        "data-sidebar": sidebarOpen ? "open" : "closed",
        "data-panes-alone": !sidebarOpen || undefined,
      }}
      style={{ "--workspace-sidebar-width": `${sidebarWidth}px` } as CSSProperties}
    >
      <WorkspaceTitlebar>
        <IconButton
          icon="sidebar"
          label={sidebarLabel}
          shortcut={frame.shortcut("toggleSidebar")}
          aria-expanded={sidebarOpen}
          aria-controls="workspace-sidebar"
          onClick={() => dispatch(toggleSidebar())}
        />
        {/* Compose lives in the sidebar; with the sidebar away it waits by the toggle. */}
        <IconButton
          className="workspace-titlebar-compose"
          icon="newSession"
          label="New Session"
          shortcut={composeShortcut}
          tabIndex={sidebarOpen ? -1 : 0}
          aria-hidden={sidebarOpen || undefined}
          onClick={compose}
        />
      </WorkspaceTitlebar>
      <Columns sidebarOpen={sidebarOpen} top={top} />
      {switcher ? (
        <SwitcherHost mode={switcher} onClose={() => setSwitcher(null)} onPick={pick} />
      ) : null}
    </WorkspaceShell>
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

/** The sidebar, its edge and the panes; renders again only when the sidebar opens or closes. */
const Columns = memo(function Columns({
  sidebarOpen,
  top,
}: {
  sidebarOpen: boolean
  top: ReactNode
}) {
  const dispatch = useWorkspaceDispatch()
  const from = useRef(0)
  return (
    <>
      <SourceList variant="tree" top={top} />
      {sidebarOpen ? (
        <ResizeEdge
          label="Resize Sidebar"
          onStart={() => {
            from.current =
              document.querySelector(".workspace-sidebar")?.getBoundingClientRect()
                .width ?? 0
          }}
          onMove={(delta) => dispatch(resizeSidebar({ width: from.current + delta }))}
        />
      ) : null}
      <PaneGrid />
    </>
  )
})
