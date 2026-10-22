/**
 * Switching between the window's tabs and opening one (#1077). A tab owns its
 * layout (`model/window-tabs.ts`): switching swaps whole layouts, so a merged
 * tab comes back split as it was left. What happens to panes inside the
 * active tab — and the merges and unmerges that follow — is `panes.ts`.
 */
import {
  focusPane as focusLayoutPane,
  paneShowing,
  singlePane,
} from "../../../split-panes/model/pane-layout"
import { activate, addTab, tabShowing } from "../../model/window-tabs"
import { withTabs, type WorkspaceState } from "../workspace-state"

/** Shows tab `tabId`, its layout as it was left; nothing for an unknown tab. */
export function switchTab(
  state: WorkspaceState,
  { tabId }: { tabId: string },
): WorkspaceState {
  if (tabId === state.tabs.activeTab) return state
  const next = activate(state.tabs, state.panes, tabId)
  return next ? withTabs(state, next) : state
}

/**
 * Brings `item` to the front: the pane showing it focused, in whichever tab
 * holds it. Nothing when no tab does.
 */
export function showTabWith(state: WorkspaceState, item: string): WorkspaceState {
  const tab = tabShowing(state.tabs, state.panes, item)
  if (!tab) return state
  const switched = switchTab(state, { tabId: tab.id })
  const panes = switched.panes
  const pane = panes && paneShowing(panes, item)
  if (!panes || !pane || panes.focused === pane.key) return switched
  return withTabs(switched, {
    tabs: switched.tabs,
    panes: focusLayoutPane(panes, pane.key),
  })
}

/** Opens `item` alone in a new tab after the active one, and shows it. */
export function openInNewTab(state: WorkspaceState, item: string): WorkspaceState {
  if (state.tabs.activeTab === null) {
    // No tab yet: the window's first layout is its first tab.
    return withTabs(state, {
      tabs: {
        tabs: [{ id: `tab-${state.tabs.nextTab}`, layout: null }],
        activeTab: `tab-${state.tabs.nextTab}`,
        nextTab: state.tabs.nextTab + 1,
      },
      panes: singlePane(item),
    })
  }
  const added = addTab(state.tabs, singlePane(item))
  const id = `tab-${state.tabs.nextTab}`
  const next = activate(added, state.panes, id)
  return next ? withTabs(state, next) : state
}
