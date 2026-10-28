/**
 * A chat's right panel: the widgets open in it, as tabs, and the one in
 * front. Each chat has its own (ADR 326's `panel` place); the panel beside
 * the panes shows the focused chat's. Chats themselves have no tabs: they
 * are moved between with the session list, the sidebar and ⌘K.
 *
 * ```text
 *   open(w)    ──▶ w is a tab (added after the last, if new) and in front
 *   show(w)    ──▶ w in front
 *   close(w)   ──▶ w's tab goes; the one after it, else before it, comes to the front;
 *                  the panel goes when it was the last
 * ```
 */
import { sameWidget, type WidgetRef } from "../../widgets/model/widget-ref"

export interface ChatPanel {
  /** The panel's tabs, in the order they were opened. Never empty. */
  readonly tabs: readonly WidgetRef[]
  /** The tab in front: always one of `tabs`. */
  readonly front: WidgetRef
}

/** `widget` in front, a tab of its own after the others if it was not one yet. */
export function openInPanel(panel: ChatPanel | undefined, widget: WidgetRef): ChatPanel {
  if (!panel) return { tabs: [widget], front: widget }
  const tab = panel.tabs.find((each) => sameWidget(each, widget))
  if (tab) return tab === panel.front ? panel : { ...panel, front: tab }
  return { tabs: [...panel.tabs, widget], front: widget }
}

/** `widget` in front; the same panel when it has no tab for it, or it is already. */
export function showInPanel(panel: ChatPanel, widget: WidgetRef): ChatPanel {
  const tab = panel.tabs.find((each) => sameWidget(each, widget))
  return !tab || tab === panel.front ? panel : { ...panel, front: tab }
}

/**
 * `widget`'s tab taken out: the tab after it comes to the front if it was in
 * front, else the one before; `null` once no tab is left. The same panel
 * when it has no tab for `widget`.
 */
export function closeInPanel(panel: ChatPanel, widget: WidgetRef): ChatPanel | null {
  const at = panel.tabs.findIndex((each) => sameWidget(each, widget))
  if (at < 0) return panel
  const tabs = panel.tabs.filter((_, index) => index !== at)
  if (tabs.length === 0) return null
  if (!sameWidget(panel.front, widget)) return { ...panel, tabs }
  return { tabs, front: tabs[at] ?? tabs[at - 1] }
}
