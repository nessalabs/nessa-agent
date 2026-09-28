/**
 * Finding one's way: choosing what the session list shows, opening a channel
 * from the sidebar, folding and unfolding the sidebar's parts, and the side
 * columns themselves.
 */
import { byRecency } from "../../model/workspace-index"
import type { WorkspaceRoom } from "../../../split-panes/model/pane-sizing"
import { chosen, fitted } from "../../../model/side-column"
import { mostPressing } from "../../model/session-groups"
import {
  clampColumn,
  foldToFit,
  sessionListLimits,
  sidebarLimits,
} from "../../model/window-fit"
import {
  channelOf,
  drawnColumns,
  keepShownFailures,
  listedSessions,
  sessionOf,
  toggled,
  type ContentView,
  type WorkspaceState,
} from "../workspace-state"
import { createDraft, openBeside, openSession } from "./panes"

/**
 * Shows the panes or the Agents overview in the content region. Asking for
 * the one already shown leaves it: the sidebar's Agents entry is a place to
 * go, like a channel, not a switch.
 */
export function showContent(
  state: WorkspaceState,
  { content }: { content: ContentView },
): WorkspaceState {
  return state.content === content ? state : keepShownFailures({ ...state, content })
}

/**
 * Going somewhere — a session, a channel, a new session,
 * another pane — or changing the panes — closing, moving, evening them out —
 * is going to the panes: the one rule every way there follows, applied by
 * the store to each of them (`adapters/store/slice.ts`). The panes stay laid
 * out under the overview, so a change asked for while it is open is measured
 * against the room they really have.
 */
export function navigated(state: WorkspaceState): WorkspaceState {
  return showContent(state, { content: "panes" })
}

/**
 * Shows a channel's sessions in the list. With the list hidden, choosing a
 * channel still shows something of it: its most pressing session opens in
 * the focused pane.
 */
export function selectChannel(
  state: WorkspaceState,
  { channelId }: { channelId: string },
): WorkspaceState {
  if (!channelOf(state, channelId)) return state
  const viewed: WorkspaceState = { ...state, view: { channelId } }
  if (drawnColumns(state.chrome).sessionList) return viewed
  const pick = mostPressing(
    listedSessions(state).filter((session) => session.channelId === channelId),
  )
  return pick ? openSession(viewed, { sessionId: pick.id }) : viewed
}

/**
 * Opens a channel from the sidebar that lists sessions inline: its newest
 * session, in place or beside, or a new session's home when it has none.
 */
export function openChannel(
  state: WorkspaceState,
  {
    channelId,
    beside = false,
    draftId,
    room,
  }: { channelId: string; beside?: boolean; draftId?: string; room?: WorkspaceRoom },
): WorkspaceState {
  if (!channelOf(state, channelId)) return state
  const disclosed = setChannelOpen({ ...state, view: { channelId } }, channelId, true)
  const latest = listedSessions(state)
    .filter((session) => session.channelId === channelId)
    .sort(byRecency)[0]
  if (latest)
    return beside
      ? openBeside(disclosed, { sessionId: latest.id, room })
      : openSession(disclosed, { sessionId: latest.id })
  if (!draftId) return disclosed
  return createDraft(disclosed, {
    draftId,
    channelId,
    beside: beside ? "right" : undefined,
    room,
  })
}

/** Brings a session into view in the sidebar: open, its section and channel unfolded. */
export function revealSession(
  state: WorkspaceState,
  { sessionId }: { sessionId: string },
): WorkspaceState {
  const session = sessionOf(state, sessionId)
  const channel = session && channelOf(state, session.channelId)
  if (!session || !channel) return state
  return setChannelOpen(
    {
      ...toggleSidebar(state, { open: true }),
      view: { channelId: channel.id },
      tree: {
        ...state.tree,
        collapsedSections: toggled(
          state.tree.collapsedSections,
          channel.sectionId,
          false,
        ),
      },
    },
    channel.id,
    true,
  )
}

export function toggleSection(
  state: WorkspaceState,
  { sectionId }: { sectionId: string },
): WorkspaceState {
  const collapsed = state.tree.collapsedSections.includes(sectionId)
  return {
    ...state,
    tree: {
      ...state.tree,
      collapsedSections: toggled(state.tree.collapsedSections, sectionId, !collapsed),
    },
  }
}

function setChannelOpen(
  state: WorkspaceState,
  channelId: string,
  open: boolean,
): WorkspaceState {
  const expandedChannels = toggled(state.tree.expandedChannels, channelId, open)
  if (expandedChannels === state.tree.expandedChannels) return state
  return { ...state, tree: { ...state.tree, expandedChannels } }
}

/** Discloses a channel's sessions, or folds them; `open` says which, else it flips. */
export function toggleChannel(
  state: WorkspaceState,
  { channelId, open }: { channelId: string; open?: boolean },
): WorkspaceState {
  return setChannelOpen(
    state,
    channelId,
    open ?? !state.tree.expandedChannels.includes(channelId),
  )
}

export function toggleShowAll(
  state: WorkspaceState,
  { channelId }: { channelId: string },
): WorkspaceState {
  const all = state.tree.showAllChannels.includes(channelId)
  return {
    ...state,
    tree: {
      ...state.tree,
      showAllChannels: toggled(state.tree.showAllChannels, channelId, !all),
    },
  }
}

/**
 * The person shows or hides the sidebar — `open`, else the other of what is
 * drawn. Their choice, so a fold the window made for room lifts with it.
 */
export function toggleSidebar(
  state: WorkspaceState,
  { open }: { open?: boolean } = {},
): WorkspaceState {
  const sidebar = chosen(state.chrome.sidebar, open)
  if (sidebar === state.chrome.sidebar) return state
  return { ...state, chrome: { ...state.chrome, sidebar } }
}

/** The person shows or hides the session list, as `toggleSidebar` does the sidebar. */
export function toggleSessionList(
  state: WorkspaceState,
  { open }: { open?: boolean } = {},
): WorkspaceState {
  const sessionList = chosen(state.chrome.sessionList, open)
  if (sessionList === state.chrome.sessionList) return state
  return { ...state, chrome: { ...state.chrome, sessionList } }
}

export function resizeSidebar(
  state: WorkspaceState,
  { width }: { width: number },
): WorkspaceState {
  const sidebarWidth = clampColumn(width, sidebarLimits)
  if (sidebarWidth === state.chrome.sidebarWidth) return state
  return { ...state, chrome: { ...state.chrome, sidebarWidth } }
}

export function resizeSessionList(
  state: WorkspaceState,
  { width }: { width: number },
): WorkspaceState {
  const sessionListWidth = clampColumn(width, sessionListLimits)
  if (sessionListWidth === state.chrome.sessionListWidth) return state
  return { ...state, chrome: { ...state.chrome, sessionListWidth } }
}

/**
 * Folds, for room, the side columns the person has open that a window this
 * wide cannot afford beside the panes — and lifts a fold once the room is
 * back, so a column the window folded returns on its own. What the person
 * chose is left as it is. Run when the window's width or the number of pane
 * columns changes. The widths are the ones drawn, which the layout
 * measured; a layout without a session list passes zero for it.
 */
export function fitToWindow(
  state: WorkspaceState,
  {
    windowWidth,
    sidebarWidth,
    sessionListWidth,
  }: { windowWidth: number; sidebarWidth: number; sessionListWidth: number },
): WorkspaceState {
  const columns = state.panes?.columns.length ?? 1
  const hasList = sessionListWidth > 0
  // What fits is asked of the columns the person has open.
  const room = foldToFit(
    {
      sidebarOpen: state.chrome.sidebar.open,
      sessionListOpen: hasList && state.chrome.sessionList.open,
      sidebarWidth,
      sessionListWidth,
    },
    windowWidth,
    columns,
  )
  const sidebar = fitted(state.chrome.sidebar, room.sidebarOpen)
  // A layout without the list leaves its fold as it was.
  const sessionList = hasList
    ? fitted(state.chrome.sessionList, room.sessionListOpen)
    : state.chrome.sessionList
  if (sidebar === state.chrome.sidebar && sessionList === state.chrome.sessionList)
    return state
  return { ...state, chrome: { ...state.chrome, sidebar, sessionList } }
}
