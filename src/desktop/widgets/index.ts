/**
 * The desktop window's widgets (ADR 326): what a plugin draws, in a message,
 * in a pane of its own, or in the window over the panes — the contract, the
 * registry the plugins are looked up in, and the hosts that draw them. It
 * knows no plugin, and imports no other vertical: the workspace imports it,
 * never the other way (`scripts/architecture/desktop-verticals.mjs`).
 *
 * ```text
 *   composition (main.tsx, dependencies.ts)
 *        │ native plugins, once ──▶ application/registry.ts ◀── app plugins, at run time (#349)
 *        ▼                                │ looked up by id (adapters/react/registry-context.tsx)
 *   WidgetRegistryProvider                ▼
 *                            ui/widget-answer.tsx ── plugin.useWidget(id) ──▶ WidgetAnswer
 *                                         │ asks model/host-table.ts what to draw
 *                                         ▼
 *            ui/inline-widget.tsx (a message's card), ui/widget-body.tsx (a pane's, the window's)
 *                                         │ the plugin's view, given its place, the host
 *                                         ▼ (ui/plugin.ts: WidgetHost) and the host context
 *                                   the workspace's chrome around them
 * ```
 *
 * An arrow points the way a plugin, or its answer, travels. `model/` holds
 * the pure parts: the reference, the states and places, and ADR 326's table
 * of what a host draws for each; `application/` the registry and a host's
 * Escape (`escape-stack.ts`), with no React in either; `adapters/` the
 * registry's React context and the host context read from the page; `ui/`
 * the contract a plugin implements and the hosts. `fixture/` is the sample
 * plugin composition registers while the sample workspace is in use.
 */
export type { WidgetRef } from "./model/widget-ref"
export { sameWidget } from "./model/widget-ref"
export type {
  HostContext,
  OpenPlace,
  WidgetAnswer,
  WidgetPlace,
  WidgetState,
} from "./model/widget-state"
export { offersWindow, widgetOrigin, widgetTitle } from "./model/host-table"
export {
  createWidgetRegistry,
  WidgetRegistryError,
  type RegisterOutcome,
  type UnregisterOutcome,
} from "./application/registry"
export { escapeStack, type EscapeStack } from "./application/escape-stack"
export type {
  AppWidgetPlugin,
  DesktopWidgetRegistry,
  NativeWidgetPlugin,
  SessionAccessoryProps,
  WidgetHost,
  WidgetPlugin,
  WidgetViewProps,
} from "./ui/plugin"
export {
  useNativePlugins,
  WidgetRegistryProvider,
} from "./adapters/react/registry-context"
export { offeredBy, WidgetAnswerOf } from "./ui/widget-answer"
export { WidgetBody, widgetBodyAttribute } from "./ui/widget-body"
export { InlineWidget } from "./ui/inline-widget"
export { samplePlugin } from "./fixture/sample-plugin"
export { samplePluginId, sampleWidgets } from "./fixture/sample-widgets"
