/**
 * The window's tabs (#1077). A tab is one chat or a merged group of chats,
 * and it owns its own pane layout: switching tabs swaps whole layouts, so a
 * merged tab comes back split exactly as it was left — sizes and focus.
 *
 * One owner per layout. The active tab's layout is the workspace's `panes`,
 * which every pane command already changes; every other tab keeps its own in
 * `layout`. The active tab's `layout` is `null` while it is active.
 *
 * ```text
 *   tabs:   [ A (layout: null) ][ B (layout: L1) ][ C (layout: L2) ]
 *   panes:    ▲ A's layout, while A is active
 * ```
 *
 * A pane change in the active tab is reconciled here (`reconcileTabs`): an
 * item that arrives in it leaves the tab it was in (a merge), and one that
 * leaves it without being let go stands as a tab of its own (an unmerge).
 */
import {
  panesOf,
  paneCount,
  paneShowing,
  removePane,
  singlePane,
  type PaneLayout,
} from "../../split-panes/model/pane-layout"

export interface WindowTab {
  readonly id: string
  /** This tab's layout while another is active; `null` while it is the active one (`panes`). */
  readonly layout: PaneLayout | null
}

/** The tabs, which is active, and the next id to mint. */
export interface WindowTabs {
  readonly tabs: readonly WindowTab[]
  readonly activeTab: string | null
  readonly nextTab: number
}

export const noTabs: WindowTabs = { tabs: [], activeTab: null, nextTab: 1 }

/** The items a layout shows, in reading order. */
export function itemsOf(layout: PaneLayout | null): string[] {
  return layout ? panesOf(layout).map((pane) => pane.item) : []
}

/** A tab's layout: the active one's is `panes`. */
export function layoutOf(
  tabs: WindowTabs,
  panes: PaneLayout | null,
  tab: WindowTab,
): PaneLayout | null {
  return tab.id === tabs.activeTab ? panes : tab.layout
}

/** The tab showing `item`, if one does. */
export function tabShowing(
  tabs: WindowTabs,
  panes: PaneLayout | null,
  item: string,
): WindowTab | undefined {
  return tabs.tabs.find((tab) => itemsOf(layoutOf(tabs, panes, tab)).includes(item))
}

/** Every item any tab shows: what the window keeps for its tabs. */
export function itemsInTabs(tabs: WindowTabs, panes: PaneLayout | null): Set<string> {
  return new Set(tabs.tabs.flatMap((tab) => itemsOf(layoutOf(tabs, panes, tab))))
}

/** Mints a tab for `layout`, inactive, after `after` — the active tab by default — or last. */
export function addTab(
  tabs: WindowTabs,
  layout: PaneLayout,
  after: string | null = tabs.activeTab,
): WindowTabs {
  const tab: WindowTab = { id: `tab-${tabs.nextTab}`, layout }
  const at = tabs.tabs.findIndex((each) => each.id === after)
  const list =
    at < 0
      ? [...tabs.tabs, tab]
      : [...tabs.tabs.slice(0, at + 1), tab, ...tabs.tabs.slice(at + 1)]
  return { ...tabs, tabs: list, nextTab: tabs.nextTab + 1 }
}

/**
 * Makes `id` the active tab: the leaving tab keeps `panes` as its own, and
 * the arriving tab's layout becomes `panes`. Nothing for an unknown tab.
 */
export function activate(
  tabs: WindowTabs,
  panes: PaneLayout | null,
  id: string,
): { tabs: WindowTabs; panes: PaneLayout | null } | null {
  const arriving = tabs.tabs.find((tab) => tab.id === id)
  if (!arriving) return null
  if (id === tabs.activeTab) return { tabs, panes }
  return {
    tabs: {
      ...tabs,
      activeTab: id,
      tabs: tabs.tabs.map((tab) =>
        tab.id === id
          ? { ...tab, layout: null }
          : tab.id === tabs.activeTab
            ? { ...tab, layout: panes }
            : tab,
      ),
    },
    panes: arriving.layout,
  }
}

/** A stored layout without `item`'s pane; `null` when it was the only one. */
function without(layout: PaneLayout, item: string): PaneLayout | null {
  const pane = paneShowing(layout, item)
  if (!pane) return layout
  return paneCount(layout) > 1 ? removePane(layout, pane.key) : null
}

/** Takes `item` out of an inactive tab; the tab goes when it was its only pane. */
export function removeFromTab(tabs: WindowTabs, tabId: string, item: string): WindowTabs {
  const list = tabs.tabs.flatMap((tab) => {
    if (tab.id !== tabId || tab.id === tabs.activeTab || !tab.layout) return [tab]
    const layout = without(tab.layout, item)
    if (layout === tab.layout) return [tab]
    return layout ? [{ ...tab, layout }] : []
  })
  return list.length === tabs.tabs.length && list.every((tab, i) => tab === tabs.tabs[i])
    ? tabs
    : { ...tabs, tabs: list }
}

/** Takes a tab out; the one after it, else before it, becomes active if it was. */
export function dropTab(
  tabs: WindowTabs,
  id: string,
): { tabs: WindowTabs; heir: WindowTab | undefined } {
  const at = tabs.tabs.findIndex((tab) => tab.id === id)
  if (at < 0) return { tabs, heir: undefined }
  const list = tabs.tabs.filter((tab) => tab.id !== id)
  const heir = id === tabs.activeTab ? (list[at] ?? list[at - 1]) : undefined
  return { tabs: { ...tabs, tabs: list }, heir }
}

/**
 * The active tab's layout changed from `before` to `after`. An item that
 * arrived leaves whichever other tab held it — a merge, done on purpose by a
 * drop, a split or Open Beside. An item that left, unless it is `released`
 * (closed, or let go) or no longer `kept`, stands as a tab of its own after
 * this one — a pane closed out of a merged group, or a chat a drop replaced.
 * The first layout the window shows gets a tab of its own.
 */
export function reconcileTabs(
  tabs: WindowTabs,
  before: PaneLayout | null,
  after: PaneLayout,
  kept: (item: string) => boolean,
  released: ReadonlySet<string> = new Set(),
): WindowTabs {
  let next: WindowTabs =
    tabs.activeTab === null
      ? {
          tabs: [...tabs.tabs, { id: `tab-${tabs.nextTab}`, layout: null }],
          activeTab: `tab-${tabs.nextTab}`,
          nextTab: tabs.nextTab + 1,
        }
      : tabs
  const was = itemsOf(before)
  const now = itemsOf(after)
  for (const item of now) {
    if (was.includes(item)) continue
    const holder = next.tabs.find(
      (tab) => tab.id !== next.activeTab && itemsOf(tab.layout).includes(item),
    )
    if (holder) next = removeFromTab(next, holder.id, item)
  }
  for (const item of was) {
    if (now.includes(item) || released.has(item) || !kept(item)) continue
    if (tabShowing(next, after, item)) continue
    next = addTab(next, singlePane(item))
  }
  return next
}
