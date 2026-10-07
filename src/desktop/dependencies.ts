/**
 * The desktop window's composition: every outside thing it talks to — the
 * workspace source, the clock, ids, the page's measure of the panes' room,
 * the storage that keeps the overview's filter — and the widget plugins it
 * draws with, built once and handed to the store (`store.ts`) and the views.
 * Overrides are explicit, never a service locator — a test passes its own
 * `workspace`.
 *
 * The workspace is the gateway's (`gatewaySource`) when the window is given
 * a way to connect to one (`gateway`), and the in-memory sample otherwise —
 * which is what the verification checks run on, but for an app's review
 * (`verification/desktop/fixtures/app-review/`), which the sample has none of.
 * The sample subagent source is joined only then; any other window's source
 * stays unread and the plugin is not registered.
 *
 * Beside a gateway's source, where apps are drawn, real servers' apps are
 * too: the source hands each view it reads to `gatewayApps`, which registers
 * an app plugin for each server a view names with a UI, and their calls go on
 * the source's own client (`client.mcpApps`, #384,
 * `widgets/app/adapters/gateway/`). The fixture app stays with the sample
 * workspace.
 *
 * ```ts
 * const dependencies = createDesktopDependencies({ gateway: () => connect(), apps })
 * const store = makeDesktopStore(dependencies)
 * ```
 */
import type { McpAppsApi } from "@nessa/client"
import {
  linkedDevicesGateway,
  mcpServersGateway,
  type LinkedDevicesClient,
  type LinkedDevicesGateway,
  type McpServersClient,
  type McpServersGateway,
} from "./settings"
import {
  createWidgetRegistry,
  fixtureAppPlugin,
  fixtureConversation,
  gatewayApps,
  platformFor,
  readPageContext,
  samplePlugin,
  type DesktopWidgetRegistry,
  type NativeWidgetPlugin,
  type SandboxOrigin,
  type Timers,
  type WidgetPlugin,
} from "./widgets"
import {
  joinSubagentSources,
  sampleSubagentKey,
  sampleSubagentSource,
  subagentsPlugin,
  unreadSubagentSource,
  type SubagentSource,
} from "./subagents"
import {
  gatewaySource,
  inMemorySource,
  measureWorkspace,
  type InMemorySource,
  rememberedFilter,
  sampleAppSession,
  sampleWidgetSession,
  type GatewayClient,
  type GatewaySource,
  type WorkspaceDependencies,
  type WorkspaceSource,
} from "./workspace"

export interface DesktopDependencies extends WorkspaceDependencies {
  /** The widget plugins, native ones registered here (ADR 326); provided to the tree by `main.tsx`. */
  readonly widgets: DesktopWidgetRegistry
  /** The gateway's stored MCP servers, for Settings; absent without a gateway. Provided by `main.tsx`. */
  readonly mcpServers: McpServersGateway | undefined
  /** Pairing and linked devices, for Settings; absent without a gateway. Provided by `main.tsx`. */
  readonly linkedDevices: LinkedDevicesGateway | undefined
  /**
   * Where a conversation's subagents are read. The sample source while the
   * sample workspace is in use; unread otherwise, so a window does not draw
   * an empty list in place of a source it has not read. Provided by `main.tsx`.
   */
  readonly subagents: SubagentSource
}

export function createDesktopDependencies(
  options: {
    workspace?: WorkspaceSource
    signInToProvider?: WorkspaceDependencies["signInToProvider"]
    providerLoginAvailable?: WorkspaceDependencies["providerLoginAvailable"]
    /**
     * Connects to the gateway whose conversations the window shows, and whose
     * servers' apps it draws where `apps` are; ignored beside `workspace`.
     */
    gateway?: () => Promise<WindowGatewayClient>
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
  } = {},
): DesktopDependencies {
  // The real clock and timers; tests pass their own.
  const now = options.now ?? (() => Date.now())
  const after = (ms: number, run: () => void) => {
    const timer = window.setTimeout(run, ms)
    return () => window.clearTimeout(timer)
  }
  // With no source given and no gateway, the window runs on the sample workspace.
  const sample = options.workspace === undefined && options.gateway === undefined
  const newId = options.newId ?? (() => crypto.randomUUID())
  const { apps, gateway } = options
  // The sample workspace, which the fixture app speaks to (#390).
  const samples = sample ? inMemorySource({ now, after }) : undefined
  // The fixture app only beside the sample workspace, never beside a gateway,
  // whose servers' apps it could otherwise stand in for.
  const subagents = sample
    ? joinSubagentSources(
        [{ key: sampleSubagentKey, source: sampleSubagentSource({ now, after }) }],
        { warn: (message) => console.warn(message) },
      )
    : unreadSubagentSource()
  const widgets = widgetRegistry(options.widgets, samples, apps, after)
  // The servers are managed on the gateway's own source's client, beside
  // its conversations; a source composition names has no client to give.
  const source =
    options.workspace === undefined && gateway
      ? gatewayWorkspace(gateway, { now, after }, widgets, apps)
      : undefined
  const workspace =
    options.workspace ?? source ?? samples ?? inMemorySource({ now, after })
  return {
    workspace,
    signInToProvider: options.signInToProvider,
    providerLoginAvailable: options.providerLoginAvailable,
    mcpServers: source
      ? mcpServersGateway({ connected: () => source.connected(), after })
      : undefined,
    linkedDevices: source
      ? linkedDevicesGateway({ connected: () => source.connected(), after })
      : undefined,
    now,
    newId,
    // The page's own layout: every command that changes the panes is held to it.
    measure: options.measure ?? (() => measureWorkspace(document)),
    // The webview's storage, where the overview's filter is kept between launches.
    overviewFilter: options.overviewFilter ?? rememberedFilter(),
    widgets,
    subagents,
  }
}

/** A gateway client the window can show conversations, draw apps, manage MCP servers, and pair devices through. */
type WindowGatewayClient = GatewayClient &
  McpServersClient &
  LinkedDevicesClient & { readonly mcpApps: McpAppsApi }

/**
 * The gateway's workspace, and — where apps are drawn — its servers' apps:
 * told each view the source reads, their calls made on the client the source
 * holds, so an app is asked on the same connection its conversation is read,
 * and each tool call and message through the source's `appCall`, so its
 * conversation is read while either waits on the person's review.
 */
function gatewayWorkspace(
  connect: () => Promise<WindowGatewayClient>,
  clock: { now: () => number; after: Timers["after"] },
  registry: DesktopWidgetRegistry,
  apps: AppsOptions | undefined,
): GatewaySource<WindowGatewayClient> {
  if (!apps) return gatewaySource({ connect, clock })
  const mcpApps = (): Promise<McpAppsApi> =>
    source.connected().then((client) => client.mcpApps)
  const source: GatewaySource<WindowGatewayClient> = gatewaySource({
    connect,
    clock,
    apps: gatewayApps({
      registry,
      mcpApps: {
        // Through the source, which reads the conversation while the call
        // may wait on a review there (#436).
        callTool: (...args) =>
          source.appCall(args[0], () => mcpApps().then((api) => api.callTool(...args))),
        readResource: (...args) => mcpApps().then((api) => api.readResource(...args)),
        // Through the source too: every message waits on the person's review
        // there (#390, D-E).
        sendMessage: (...args) =>
          source.appCall(args[0], () =>
            mcpApps().then((api) => api.sendMessage(...args)),
          ),
        // Nothing waits on the person: no read is started for it.
        updateModelContext: (...args) =>
          mcpApps().then((api) => api.updateModelContext(...args)),
        fetchResource: (...args) => mcpApps().then((api) => api.fetchResource(...args)),
        releaseApp: (...args) => mcpApps().then((api) => api.releaseApp(...args)),
      },
      ports: appPorts(apps, clock.after),
    }),
  })
  return source
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
  samples: InMemorySource | undefined,
  apps: AppsOptions | undefined,
  after: Timers["after"],
): DesktopWidgetRegistry {
  const sample = samples !== undefined
  // The sample plugin only beside the sample workspace, whose session its widgets belong to.
  const registry = createWidgetRegistry<WidgetPlugin>(
    natives ?? (sample ? [samplePlugin(sampleWidgetSession), subagentsPlugin()] : []),
  )
  // And the fixture MCP App beside it; real servers' apps come through the
  // gateway (`gatewayApps`).
  if (samples && apps)
    registry.register(
      fixtureAppPlugin({
        sessionId: sampleAppSession,
        sandbox: apps.sandbox,
        timers: { after },
        newId: () => crypto.randomUUID(),
        page: () => readPageContext(document, apps.platform),
        // Its messages land in the sample workspace, written by the app.
        conversation: fixtureConversation(samples.appMessage),
      }),
    )
  return registry
}
