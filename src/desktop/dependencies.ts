/**
 * The desktop window's composition: every outside thing it talks to — the
 * workspace source, the clock, ids, the page's measure of the panes' room,
 * the storage that keeps the overview's filter — and the widget plugins it
 * draws with, built once and handed to the store (`store.ts`) and the views.
 * Overrides are explicit, never a service locator — a test or a future
 * gateway adapter passes its own `workspace`.
 *
 * ```ts
 * const dependencies = createDesktopDependencies({ workspace: gatewaySource })
 * const store = makeDesktopStore(dependencies)
 * ```
 */
import {
  createWidgetRegistry,
  fixtureAppPlugin,
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
  return {
    workspace,
    now,
    newId: options.newId ?? (() => crypto.randomUUID()),
    // The page's own layout: every command that changes the panes is held to it.
    measure: options.measure ?? (() => measureWorkspace(document)),
    // The webview's storage, where the overview's filter is kept between launches.
    overviewFilter: options.overviewFilter ?? rememberedFilter(),
    widgets: widgetRegistry(options.widgets, sample, options.apps, after),
  }
}

/** Where this window draws MCP Apps, as the host decides it. */
interface AppsOptions {
  readonly sandbox: SandboxOrigin | undefined
  readonly platform: ReturnType<typeof platformFor>
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
  // And the fixture MCP App beside it: real servers' apps reach the window
  // once `McpAppServer` is wired to the gateway's `client.mcpApps` (#384).
  if (sample && apps)
    registry.register(
      fixtureAppPlugin({
        sessionId: sampleAppSession,
        sandbox: apps.sandbox,
        timers: { after },
        page: () => readPageContext(document, apps.platform),
      }),
    )
  return registry
}
