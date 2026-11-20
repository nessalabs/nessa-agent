/**
 * Chat context: what a chat runs on and what it made, the first tab of its
 * right panel. Every chat has one, so it is never opened or closed like a
 * widget: the panel's tabs are Chat context and then the widgets opened in
 * it (`panelTabs`). Bringing it to the front stores it as a tab like any
 * other (`openInPanel`), which is how the panel remembers it is in front.
 *
 * ```text
 *   no widgets open        ──▶ [Chat context]                 front: Chat context
 *   widgets a, b; b front  ──▶ [Chat context, a, b]           front: b
 *   Chat context chosen    ──▶ [Chat context, a, b]           front: Chat context
 * ```
 */
import type { WidgetRef } from "../../widgets/model/widget-ref"
import type { ChatPanel } from "./chat-panel"

/** The plugin name Chat context's tab is stored under. No plugin draws it: the panel does. */
export const chatContextPlugin = "chat-context"

/** `sessionId`'s Chat context, as a panel tab. */
export function chatContextTab(sessionId: string): WidgetRef {
  return { plugin: chatContextPlugin, id: sessionId }
}

export function isChatContextTab(widget: WidgetRef): boolean {
  return widget.plugin === chatContextPlugin
}

/**
 * The tabs `sessionId`'s panel draws, Chat context first, and the one in
 * front: the panel's own, or Chat context when nothing else was opened.
 */
export function panelTabs(
  sessionId: string,
  panel: ChatPanel | null | undefined,
): { tabs: readonly WidgetRef[]; front: WidgetRef; widgets: readonly WidgetRef[] } {
  const context = chatContextTab(sessionId)
  const widgets = panel ? panel.tabs.filter((tab) => !isChatContextTab(tab)) : []
  const front = panel && !isChatContextTab(panel.front) ? panel.front : context
  return { tabs: [context, ...widgets], front, widgets }
}

/**
 * `panel` with Chat context's tab first, where the panel draws it, so the tab
 * that comes to the front when one closes is the one beside it on screen.
 */
export function contextFirst(panel: ChatPanel): ChatPanel {
  const at = panel.tabs.findIndex(isChatContextTab)
  if (at <= 0) return panel
  const tabs = [panel.tabs[at], ...panel.tabs.filter((_, index) => index !== at)]
  return { ...panel, tabs }
}
