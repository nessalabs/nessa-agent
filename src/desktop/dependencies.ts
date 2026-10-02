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
  fixtureServer,
  readPageContext,
  samplePlugin,
  type DesktopWidgetRegistry,
  type NativeWidgetPlugin,
  type SandboxOrigin,
  type WidgetPlugin,
} from "./widgets"
import {
  inMemorySource,
  mcpAppPlugin,
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
     * With no sandbox, no app can be shown, and each says so.
     */
    apps?: { readonly sandbox?: SandboxOrigin; readonly platform: "web" | "desktop" }
  } = {},
): DesktopDependencies {
  // The real clock and timers; tests pass their own.
  const now = options.now ?? (() => Date.now())
  // With no source given, the window runs on the sample workspace.
  const sample = options.workspace === undefined
  const workspace =
    options.workspace ??
    inMemorySource({
      now,
      after: (ms, run) => {
        const timer = window.setTimeout(run, ms)
        return () => window.clearTimeout(timer)
      },
    })
  return {
    workspace,
    now,
    newId: options.newId ?? (() => crypto.randomUUID()),
    // The page's own layout: every command that changes the panes is held to it.
    measure: options.measure ?? (() => measureWorkspace(document)),
    // The webview's storage, where the overview's filter is kept between launches.
    overviewFilter: options.overviewFilter ?? rememberedFilter(),
    widgets: widgetRegistry(options.widgets, sample, options.apps),
  }
}

function widgetRegistry(
  natives: readonly NativeWidgetPlugin[] | undefined,
  sample: boolean,
  apps:
    | { readonly sandbox?: SandboxOrigin; readonly platform: "web" | "desktop" }
    | undefined,
): DesktopWidgetRegistry {
  // The sample plugin only beside the sample workspace, whose session its widgets belong to.
  const registry = createWidgetRegistry<WidgetPlugin>(
    natives ?? (sample ? [samplePlugin(sampleWidgetSession)] : []),
  )
  // And the fixture MCP App beside it, until the gateway reports servers with UI (#348).
  if (sample)
    registry.register(
      fixtureAppPlugin({
        id: mcpAppPlugin(fixtureServer),
        sessionId: sampleAppSession,
        sandbox: apps?.sandbox,
        timers: {
          after: (ms, run) => {
            const timer = setTimeout(run, ms)
            return () => clearTimeout(timer)
          },
        },
        page: () => readPageContext(document, apps?.platform ?? "web"),
      }),
    )
  return registry
}
