/**
 * The plugins the window draws widgets with (ADR 326), looked up by id and by
 * nothing else. Native plugins are registered once, in composition
 * (`createWidgetRegistry`); `app` plugins — MCP Apps (ADR 344) — are
 * registered and unregistered while the window runs, as the gateway reports
 * servers with UI. Two plugins under one id is an error either way: a
 * `WidgetRegistryError` at composition, a typed refusal at run time
 * (`registry.test.ts`).
 *
 * Generic over the plugin, which the UI defines (`ui/plugin.ts`): a use case
 * here may not name React.
 */

/** What the registry needs of a plugin: its id, and which kind it is. */
export interface RegisteredPlugin {
  readonly id: string
  readonly kind: "native" | "app"
}

/** Composition registered two plugins under one id: the window does not start. */
export class WidgetRegistryError extends Error {
  constructor(readonly duplicates: readonly string[]) {
    super(`Two widget plugins share an id: ${duplicates.join(", ")}`)
    this.name = "WidgetRegistryError"
  }
}

/** What became of a run-time registration. */
export type RegisterOutcome =
  | { readonly kind: "registered" }
  | { readonly kind: "refused"; readonly reason: "duplicate-id" }

/** What became of a run-time unregistration: only an `app` plugin goes. */
export type UnregisterOutcome =
  | { readonly kind: "unregistered" }
  | { readonly kind: "refused"; readonly reason: "unknown-id" | "native" }

export interface WidgetRegistry<Plugin extends RegisteredPlugin> {
  /** The plugin registered under `id`, if any. */
  plugin(id: string): Plugin | undefined
  /** The native plugins, in the order composition gave them; the same array for the window's life. */
  natives(): readonly Extract<Plugin, { kind: "native" }>[]
  /** Registers an `app` plugin, unless its id is taken. */
  register(plugin: Extract<Plugin, { kind: "app" }>): RegisterOutcome
  /** Unregisters an `app` plugin; a native one stays for the window's life. */
  unregister(id: string): UnregisterOutcome
  /** Tells `listener` of every registration and unregistration; returns its stop. */
  subscribe(listener: () => void): () => void
}

/** A registry holding `natives`; refuses two under one id. */
export function createWidgetRegistry<Plugin extends RegisteredPlugin>(
  natives: readonly Extract<Plugin, { kind: "native" }>[],
): WidgetRegistry<Plugin> {
  const plugins = new Map<string, Plugin>()
  const duplicates = new Set<string>()
  for (const plugin of natives) {
    if (plugins.has(plugin.id)) duplicates.add(plugin.id)
    plugins.set(plugin.id, plugin)
  }
  if (duplicates.size > 0) throw new WidgetRegistryError([...duplicates])
  const nativeList = Object.freeze([...natives])
  const listeners = new Set<() => void>()
  const changed = () => {
    for (const listener of [...listeners]) listener()
  }
  return {
    plugin: (id) => plugins.get(id),
    natives: () => nativeList,
    register: (plugin) => {
      if (plugins.has(plugin.id)) return { kind: "refused", reason: "duplicate-id" }
      plugins.set(plugin.id, plugin)
      changed()
      return { kind: "registered" }
    },
    unregister: (id) => {
      const plugin = plugins.get(id)
      if (!plugin) return { kind: "refused", reason: "unknown-id" }
      if (plugin.kind === "native") return { kind: "refused", reason: "native" }
      plugins.delete(id)
      changed()
      return { kind: "unregistered" }
    },
    subscribe: (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
  }
}
