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
 * The files a conversation holds are read back on the same client
 * (`client.attachments`, `heldFiles`), verified, through the conversation
 * vertical's reader, which the panel reads them through too.
 *
 * ```ts
 * const dependencies = createDesktopDependencies({ gateway: () => connect(), apps })
 * const store = makeDesktopStore(dependencies)
 * ```
 */
import { gatewayAgentInstallations } from "../onboarding/adapters/agent-installations"
import type { AgentInstallations } from "../onboarding/application/agent-installations"
import type { AgentsApi, AttachmentApi, McpAppsApi } from "@nessa/client"
import {
  bindWaitingReviewNotices,
  browserReviewNotifier,
  hostReviewNotices,
  openReviewEvent,
  waitingReviewNotices,
  type HostNotices,
  type WaitingReview,
} from "./workspace/adapters/gateway/review-notices"
import {
  heldFileReader,
  HeldFileReadError,
  monotonicWallClock,
  type HeldFileReader,
} from "../conversation"
import {
  linkedDevicesGateway,
  mcpServersGateway,
  nativeHooksGateway,
  permissionRulesGateway,
  type LinkedDevicesClient,
  type LinkedDevicesGateway,
  type McpServersClient,
  type McpServersGateway,
  type NativeHooksClient,
  type NativeHooksGateway,
  type PermissionRulesClient,
  type PermissionRulesGateway,
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
  createQuestionFlights,
  defaultGatewayTiming,
  gatewaySource,
  inMemorySource,
  measureWorkspace,
  paintedRoom,
  type InMemorySource,
  rememberedFilter,
  inMemorySharing,
  sharingGateway,
  windowPublisher,
  sampleAppSession,
  sampleWidgetSession,
  type GatewayClient,
  type GatewaySource,
  type SharingClient,
  type WindowPublishClient,
  type WorkspaceDependencies,
  type WorkspaceSource,
} from "./workspace"

export interface DesktopDependencies extends WorkspaceDependencies {
  readonly agentInstallations: AgentInstallations | undefined
  /** The widget plugins, native ones registered here (ADR 326); provided to the tree by `main.tsx`. */
  readonly widgets: DesktopWidgetRegistry
  /** The gateway's stored MCP servers, for Settings; absent without a gateway. Provided by `main.tsx`. */
  readonly mcpServers: McpServersGateway | undefined
  /** Pairing and linked devices, for Settings; absent without a gateway. Provided by `main.tsx`. */
  readonly linkedDevices: LinkedDevicesGateway | undefined
  /** Claude and Codex native-hook choices, for Settings; absent without a gateway. Provided by `main.tsx`. */
  readonly nativeHooks: NativeHooksGateway | undefined
  /** Remembered permission rules, for Settings; absent without a gateway. Provided by `main.tsx`. */
  readonly permissionRules: PermissionRulesGateway | undefined
  /**
   * How the window reads back a file a conversation holds — one its agent
   * published — verified, for the transcript to show and save. Through the
   * gateway's own client where the workspace is the gateway's; a workspace
   * that is not holds no files, and says so. Provided by `main.tsx`.
   */
  readonly heldFiles: HeldFileReader
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
    /**
     * Where a session is shared with linked devices, beside a given
     * `workspace`; the gateway's own and the sample's are composed here.
     */
    sharing?: WorkspaceDependencies["sharing"]
    /** The native widget plugins; two under one id stop composition (`WidgetRegistryError`). */
    widgets?: readonly NativeWidgetPlugin[]
    /**
     * Where MCP Apps are drawn: the sandbox proxy's origin, which the host
     * decides (`main.tsx`), and whether this is the desktop app or a browser.
     * With no sandbox, no app can be shown, and each says so; with no `apps`
     * at all, no app plugin is registered.
     */
    apps?: AppsOptions
    /**
     * The desktop host's notices for waiting reviews, where the window runs
     * in the app (`main.tsx`). Without them, a waiting review is the
     * browser's notification, which only a browser preview has.
     */
    reviewNotices?: HostNotices
  } = {},
): DesktopDependencies {
  // The real clock and timers; tests pass their own. The clock is a steady
  // wall timeline, so a countdown does not jump with an OS clock correction.
  const now = options.now ?? monotonicWallClock()
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
      ? gatewayWorkspace(gateway, { now, after }, widgets, apps, options.reviewNotices)
      : undefined
  const workspace =
    options.workspace ?? source ?? samples ?? inMemorySource({ now, after })
  return {
    workspace,
    agentInstallations: source
      ? gatewayAgentInstallations(
          () => source.connected().then((client) => client.agents),
          newId,
          // Setup has an explicit refresh; transcript updates are not connection events.
          () => () => {},
        )
      : undefined,
    signInToProvider: options.signInToProvider,
    providerLoginAvailable: options.providerLoginAvailable,
    mcpServers: source
      ? mcpServersGateway({ connected: () => source.connected(), after })
      : undefined,
    linkedDevices: source
      ? linkedDevicesGateway({ connected: () => source.connected(), after })
      : undefined,
    nativeHooks: source
      ? nativeHooksGateway({ connected: () => source.connected() })
      : undefined,
    permissionRules: source
      ? permissionRulesGateway({ connected: () => source.connected() })
      : undefined,
    // Shared on the gateway's own client; the sample shares in memory, and a
    // window with neither offers no Share.
    sharing: source
      ? sharingGateway({ connected: () => source.connected() })
      : samples
        ? inMemorySharing({ now })
        : options.sharing,
    // What the window shows, for the person's agents: only a gateway has any.
    windowPublisher: source
      ? windowPublisher({
          connected: () => source.connected(),
          after,
          // A window that could not say what it shows says it again on the source's own retry.
          retryMs: defaultGatewayTiming.retryMs,
        })
      : undefined,
    now,
    newId,
    // The page's own layout: every command that changes the panes is held to it.
    measure: options.measure ?? (() => measureWorkspace(document)),
    paintedRoom: options.paintedRoom ?? (options.measure ? undefined : paintedRoom),
    // The webview's storage, where the overview's filter is kept between launches.
    overviewFilter: options.overviewFilter ?? rememberedFilter(),
    questionFlights: createQuestionFlights(),
    widgets,
    subagents,
    heldFiles: source
      ? heldFileReader(() => source.connected().then((client) => client.attachments))
      : // The sample's and a given source's sessions name no file anybody holds.
        async () => {
          throw new HeldFileReadError("not-held")
        },
  }
}

/** A gateway client the window can show conversations, draw apps, read held files, manage MCP servers, pair devices, set native hooks and share sessions through. */
type WindowGatewayClient = GatewayClient &
  McpServersClient &
  LinkedDevicesClient &
  NativeHooksClient &
  PermissionRulesClient &
  SharingClient &
  WindowPublishClient & {
    readonly agents: AgentsApi
    readonly mcpApps: McpAppsApi
    readonly attachments: AttachmentApi
  }

/** Notices for reviews that are waiting, replaced by each snapshot the gateway sends. */
function reviewWaiting(
  host: HostNotices | undefined,
): (reviews: readonly WaitingReview[]) => void {
  const notices = host
    ? hostReviewNotices(host)
    : waitingReviewNotices(
        browserReviewNotifier((conversationId) => {
          window.dispatchEvent(openReviewEvent(conversationId))
        }),
      )
  bindWaitingReviewNotices(notices.showWaiting)
  return notices.apply
}

/**
 * The gateway's workspace, and — where apps are drawn — its servers' apps:
 * told each view the source applies, their calls made on the client the
 * source holds, so an app is asked on the same connection its conversation is
 * followed on, and each tool call and message through the source's `appCall`,
 * so its conversation is followed while either waits on the person's review.
 */
function gatewayWorkspace(
  connect: () => Promise<WindowGatewayClient>,
  clock: { now: () => number; after: Timers["after"] },
  registry: DesktopWidgetRegistry,
  apps: AppsOptions | undefined,
  reviewNotices: HostNotices | undefined,
): GatewaySource<WindowGatewayClient> {
  const onReviewWaiting = reviewWaiting(reviewNotices)
  if (!apps) return gatewaySource({ connect, clock, onReviewWaiting })
  const mcpApps = (): Promise<McpAppsApi> =>
    source.connected().then((client) => client.mcpApps)
  const source: GatewaySource<WindowGatewayClient> = gatewaySource({
    connect,
    clock,
    onReviewWaiting,
    apps: gatewayApps({
      registry,
      mcpApps: {
        // Through the source, which follows the conversation the call may
        // wait on a review in (#436).
        callTool: (...args) =>
          source.appCall(args[0], () => mcpApps().then((api) => api.callTool(...args))),
        readResource: (...args) => mcpApps().then((api) => api.readResource(...args)),
        // Through the source too: every message waits on the person's review
        // there (#390, D-E).
        sendMessage: (...args) =>
          source.appCall(args[0], () =>
            mcpApps().then((api) => api.sendMessage(...args)),
          ),
        // Nothing waits on the person: it follows nothing.
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
