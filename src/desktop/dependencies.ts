/**
 * The desktop window's composition: every outside thing it talks to — the
 * workspace source, the clock, ids, the page's measure of the panes' room,
 * the storage that keeps the overview's filter — and the widget plugins it
 * draws with, built once and handed to the store (`store.ts`) and the views.
 * Overrides are explicit, never a service locator — a test or a future
 * gateway adapter passes its own `workspace`.
 *
 * With a gateway's `client.mcpApps`, real servers' apps are drawn: an app
 * plugin is registered for each server a conversation's view names with a UI,
 * and the source hands each view's tools to `gatewayApps.observe` (#384,
 * `widgets/app/adapters/gateway/`). The fixture app stays with the sample
 * workspace.
 *
 * ```ts
 * const dependencies = createDesktopDependencies({
 *   workspace: gatewaySource,
 *   gateway: { mcpApps: client.mcpApps },
 *   apps,
 * })
 * const store = makeDesktopStore(dependencies)
 * ```
 */
import type { McpAppsApi } from "@nessa/client"
import {
  createWidgetRegistry,
  fixtureAppPlugin,
  gatewayApps,
  platformFor,
  readPageContext,
  samplePlugin,
  type DesktopWidgetRegistry,
  type GatewayApps,
  type NativeWidgetPlugin,
  type SandboxOrigin,
  type Timers,
  type WidgetPlugin,
} from "./widgets"
import {
  inMemorySource,
  measureWorkspace,
  rememberedFilter,
  sampleAppSession,
  sampleWidgetSession,
  type WorkspaceDependencies,
  type WorkspaceSource,
} from "./workspace"

export interface DesktopDependencies extends WorkspaceDependencies {
  /** The widget plugins, native ones registered here (ADR 326); provided to the tree by `main.tsx`. */
  readonly widgets: DesktopWidgetRegistry
  /**
   * Where a gateway source reports each conversation view's tools, so the
   * apps of their servers are registered and their calls read, and each
   * conversation that goes; absent unless both a gateway and `apps` are given.
   */
  readonly gatewayApps?: GatewayApps
}

export function createDesktopDependencies(
  options: {
    workspace?: WorkspaceSource
    now?: () => number
    newId?: () => string
    measure?: WorkspaceDependencies["measure"]
    overviewFilter?: WorkspaceDependencies["overviewFilter"]
    /** The native widget plugins; two under one id stop composition (`WidgetRegistryError`). */
    widgets?: readonly NativeWidgetPlugin[]
    /**
     * Where MCP Apps are drawn: the sandbox proxy's origin, which the host
     * decides (`main.tsx`), and whether this is the desktop app or a browser.
     * With no sandbox, no app can be shown, and each says so; with no `apps`
     * at all, no app plugin is registered.
     */
    apps?: AppsOptions
    /** The gateway's MCP App calls (`client.mcpApps`): real servers' apps reach the window through them. */
    gateway?: { readonly mcpApps: McpAppsApi }
  } = {},
): DesktopDependencies {
  // The real clock and timers; tests pass their own.
  const now = options.now ?? (() => Date.now())
  const after = (ms: number, run: () => void) => {
    const timer = window.setTimeout(run, ms)
    return () => window.clearTimeout(timer)
  }
  // With no source given, the window runs on the sample workspace.
  const sample = options.workspace === undefined
  const workspace = options.workspace ?? inMemorySource({ now, after })
  const newId = options.newId ?? (() => crypto.randomUUID())
  const { apps, gateway } = options
  // The fixture app only beside the sample workspace, and never beside a
  // gateway, whose servers' apps it could otherwise stand in for.
  const widgets = widgetRegistry(
    options.widgets,
    sample,
    gateway ? undefined : apps,
    after,
  )
  return {
    workspace,
    now,
    newId,
    // The page's own layout: every command that changes the panes is held to it.
    measure: options.measure ?? (() => measureWorkspace(document)),
    // The webview's storage, where the overview's filter is kept between launches.
    overviewFilter: options.overviewFilter ?? rememberedFilter(),
    widgets,
    ...(gateway && apps
      ? {
          gatewayApps: gatewayApps({
            registry: widgets,
            mcpApps: gateway.mcpApps,
            ports: appPorts(apps, after),
          }),
        }
      : {}),
  }
}

/** Where this window draws MCP Apps, as the host decides it. */
interface AppsOptions {
  readonly sandbox: SandboxOrigin | undefined
  readonly platform: ReturnType<typeof platformFor>
}

/**
 * What every app's view is given by the window, whichever server it is. Its
 * mounts' ids are the protocol's lowercase UUIDs, whatever ids the workspace
 * is given.
 */
function appPorts(apps: AppsOptions, after: Timers["after"]) {
  return {
    timers: { after },
    newId: () => crypto.randomUUID(),
    ...(apps.sandbox ? { sandbox: apps.sandbox } : {}),
    hostInfo: { name: "Nessa", version: "desktop" },
    page: () => readPageContext(document, apps.platform),
  }
}

function widgetRegistry(
  natives: readonly NativeWidgetPlugin[] | undefined,
  sample: boolean,
  apps: AppsOptions | undefined,
  after: Timers["after"],
): DesktopWidgetRegistry {
  // The sample plugin only beside the sample workspace, whose session its widgets belong to.
  const registry = createWidgetRegistry<WidgetPlugin>(
    natives ?? (sample ? [samplePlugin(sampleWidgetSession)] : []),
  )
  // And the fixture MCP App beside it; real servers' apps come through the
  // gateway (`gatewayApps`).
  if (sample && apps)
    registry.register(
      fixtureAppPlugin({
        sessionId: sampleAppSession,
        sandbox: apps.sandbox,
        timers: { after },
        newId: () => crypto.randomUUID(),
        page: () => readPageContext(document, apps.platform),
      }),
    )
  return registry
}
