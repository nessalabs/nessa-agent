/**
 * Widgets: what a conversation carries besides its words. The transcript
 * holds a `WidgetRef`; a plugin registered in `ui/registry.tsx` draws it —
 * inline where the conversation mentions it, and opened beside the
 * conversation by the pane's `WidgetHost`.
 *
 * ```text
 *   transcript Part { kind: "widget" } ──▶ InlineWidget ──▶ plugin.Inline
 *                                              │ open
 *                                              ▼
 *   Pane ── WidgetHost (one per pane) ──▶ WidgetBeside ──▶ plugin.Surface
 * ```
 *
 * The only plugin today is `experiment` (`../experiments`).
 */
export { sameWidget, type WidgetRef } from "./model/widget"
export { WidgetHost, useWidgetHost } from "./ui/widget-host"
export {
  InlineWidget,
  WidgetBeside,
  WidgetInPane,
  widgetKind,
  useWidgetSession,
  useWidgetTitle,
  useWidgetTrail,
} from "./ui/widget-views"
