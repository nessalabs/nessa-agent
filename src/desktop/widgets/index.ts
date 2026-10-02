/**
 * The desktop window's widgets (ADR 326): what a plugin draws, in a message,
 * in a pane of its own, or in the window over the panes — the contract, the
 * registry the plugins are looked up in, and the hosts that draw them. It
 * knows no plugin, and imports no other vertical: the workspace imports it,
 * never the other way (`scripts/architecture/desktop-verticals.mjs`).
 *
 * ```text
 *   composition (main.tsx, dependencies.ts)
 *        │ native plugins, once ──▶ application/registry.ts ◀── app plugins, at run time
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
 *
 * `app/` is the `app` kind's renderer (ADR 344, #349), the hosts' view for an
 * MCP App, laid out the same way:
 *
 * ```text
 *   ui/widget-body.tsx, ui/inline-widget.tsx ──▶ app/ui/app-view.tsx ── one bridge per view
 *                                                    │ the proxy's <iframe>, on the sandbox origin
 *                                                    ▼
 *   app/adapters/dom/frame-transport.ts ── this frame's window and origin only ──▶ app/model/messages.ts
 *                                                    │ a typed copy of each message
 *                                                    ▼
 *   app/application/bridge.ts ── app/model/lifecycle.ts, tool-call.ts, host-context.ts, places.ts, csp.ts
 *                                                    │ the app's requests, by port
 *                                                    ▼
 *   app/application/ports.ts: the server (#348), the calls, the conversation, links, downloads, timers
 * ```
 *
 * The first arrow is what draws what; the rest point the way a message
 * travels. The proxy is
 * `app/sandbox/proxy.html`; `app/fixture/` is a fixture server's app,
 * registered beside the sample workspace.
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
export type { McpAppPorts, SandboxOrigin, Timers } from "./app/application/ports"
export { readPageContext } from "./app/adapters/dom/page-context"
export { platformFor, sandboxFor } from "./app/adapters/dom/sandbox-origin"
export { fixtureAppPlugin } from "./app/fixture/fixture-plugin"
export { fixtureServer } from "./app/fixture/fixture-widgets"
export { samplePlugin } from "./fixture/sample-plugin"
export { samplePluginId, sampleWidgets } from "./fixture/sample-widgets"
