/**
 * A change one of the person's agents, or their own CLI, asks of this window
 * (`model/window-change.ts`), carried out through the commands a click or a
 * shortcut dispatches, so it follows the same rules: opening a session on
 * screen already focuses it, opening one beside where the room will not hold
 * it shows it in the focused pane, closing the last pane leaves a new
 * session, and so on.
 *
 * ```text
 * change ─▶ refused? (not listed, not on screen, no such tab, no list here, no room…)
 *        ─▶ the command a person's click dispatches
 *        ─▶ what the window shows, before against after ─▶ applied | unchanged
 *        ─▶ the right panel: its layout says when it is drawn (`panelShown`),
 *           so its outcome waits for that, or `covered` past PANEL_DRAWN_MS
 * ```
 *
 * The outcome is said once the window has had its say: the caller
 * (`adapters/gateway/window-commands.ts`) waits for the gateway to hold what
 * it publishes after, and answers with that revision.
 */
import { sameWidget } from "../../../widgets/model/widget-ref"
import { focusedPane } from "../../../split-panes/model/pane-layout"
import { canOpenBeside as canOpenBesideUseCase } from "../../application/usecases/panes"
import { shownWindow } from "../../application/usecases/window"
import {
  entry,
  paneShowingSession,
  sessionIn,
  sessionOf,
  type WorkspaceState,
} from "../../application/workspace-state"
import type { WindowChange, WindowChangeOutcome } from "../../model/window-change"
import {
  focusPane,
  openSession,
  setPanel,
  showContent,
  showPanelTab,
  toggleSessionList,
  toggleSidebar,
  type WorkspaceCommand,
} from "./commands"
import { closePane, openBeside } from "./pane-commands"

/** How long the layout has to draw the right panel once it is set open. */
export const PANEL_DRAWN_MS = 1500

/** What the store gives a window change: the window's own store. */
export interface WindowChangeStore {
  dispatch<R>(command: WorkspaceCommand<R>): R
  getState(): { workspace: WorkspaceState }
  subscribe(listener: () => void): () => void
}

type Carried =
  | WindowChangeOutcome
  /** Set the panel; its outcome is the layout's to say. */
  | { readonly kind: "panel"; readonly open: boolean }

/**
 * Carries `change` out on `store`, and says what came of it once the window
 * shows it. `after` is the window's timer.
 */
export async function carryOutWindowChange(
  store: WindowChangeStore,
  change: WindowChange,
  after: (ms: number, run: () => void) => () => void,
): Promise<WindowChangeOutcome> {
  const carried = store.dispatch(carryOut(change))
  if (carried.kind !== "panel") return carried
  const drawn = await panelDrawn(store, carried.open, after)
  return drawn ? { kind: "applied" } : { kind: "refused", code: "covered" }
}

/** The change, dispatched; what came of it, or that the panel's layout is to say. */
export function carryOut(change: WindowChange): WorkspaceCommand<Carried> {
  return (dispatch, getState) => {
    const state = getState().workspace
    const before = JSON.stringify(shownWindow(state))
    const outcome = (): WindowChangeOutcome => ({
      kind:
        JSON.stringify(shownWindow(getState().workspace)) === before
          ? "unchanged"
          : "applied",
    })
    switch (change.kind) {
      case "sidebar":
        dispatch(toggleSidebar({ open: change.open }))
        return outcome()
      case "sessionList":
        // The sidebar arrangement lists sessions in the sidebar: there is no list to show.
        if (state.arrangement === "sidebar")
          return { kind: "refused", code: "no_session_list" }
        dispatch(toggleSessionList({ open: change.open }))
        return outcome()
      case "panel":
        if (state.panelOpen === change.open) return { kind: "unchanged" }
        dispatch(setPanel({ open: change.open }))
        return { kind: "panel", open: change.open }
      case "panelTab": {
        const owner = state.panes ? sessionIn(focusedPane(state.panes)) : undefined
        if (owner === undefined) return { kind: "refused", code: "no_chat_focused" }
        const panel = entry(state.panels, owner)
        if (!panel?.tabs.some((tab) => sameWidget(tab, change.widget)))
          return { kind: "refused", code: "tab_not_found" }
        dispatch(showPanelTab({ sessionId: owner, widget: change.widget }))
        if (state.panelOpen) return outcome()
        // `covered` past PANEL_DRAWN_MS says the tab is in front but not on
        // screen: what the agent asked for is not what the person sees.
        dispatch(setPanel({ open: true }))
        return { kind: "panel", open: true }
      }
      case "show":
        dispatch(showContent({ content: change.view }))
        return outcome()
      case "open": {
        if (!sessionOf(state, change.sessionId))
          return { kind: "refused", code: "conversation_not_listed" }
        const shown = state.panes && paneShowingSession(state.panes, change.sessionId)
        if (change.place === "here" || shown) {
          dispatch(openSession({ sessionId: change.sessionId }))
          return outcome()
        }
        if (change.place === "beside") {
          dispatch(openBeside({ sessionId: change.sessionId }))
          return outcome()
        }
        const side = change.place === "right" ? "right" : "bottom"
        return dispatch((_dispatch, _getState, { measure }) => {
          const room = measure()
          if (!canOpenBesideUseCase(state, { side, room }))
            return { kind: "refused", code: "no_room" } as const
          dispatch(openBeside({ sessionId: change.sessionId, side, replace: false }))
          return outcome()
        })
      }
      case "focus":
      case "close": {
        const pane = state.panes && paneShowingSession(state.panes, change.sessionId)
        if (!pane) return { kind: "refused", code: "not_on_screen" }
        if (change.kind === "focus") dispatch(focusPane({ pane: pane.key }))
        else dispatch(closePane({ pane: pane.key }))
        return outcome()
      }
    }
  }
}

/** Whether the layout says the panel is drawn `open` within PANEL_DRAWN_MS. */
function panelDrawn(
  store: WindowChangeStore,
  open: boolean,
  after: (ms: number, run: () => void) => () => void,
): Promise<boolean> {
  return new Promise((resolve) => {
    if (store.getState().workspace.panelOpen === open) return resolve(true)
    let cancel = () => {}
    const unsubscribe = store.subscribe(() => {
      if (store.getState().workspace.panelOpen !== open) return
      unsubscribe()
      cancel()
      resolve(true)
    })
    cancel = after(PANEL_DRAWN_MS, () => {
      unsubscribe()
      resolve(store.getState().workspace.panelOpen === open)
    })
  })
}
