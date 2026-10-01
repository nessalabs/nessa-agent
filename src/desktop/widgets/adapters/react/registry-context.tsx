/**
 * The window's widget registry, provided to the tree by composition
 * (`main.tsx`) and read by the hosts. A tree with no registry provided — a
 * test of something else — has no plugins, so every widget in it is drawn as
 * one the window cannot show; nothing is pretended.
 */
import { createContext, useCallback, useContext, useSyncExternalStore } from "react"
import type { ReactNode } from "react"
import type {
  DesktopWidgetRegistry,
  NativeWidgetPlugin,
  WidgetPlugin,
} from "../../ui/plugin"

const RegistryContext = createContext<DesktopWidgetRegistry | null>(null)

export function WidgetRegistryProvider({
  registry,
  children,
}: {
  registry: DesktopWidgetRegistry
  children: ReactNode
}) {
  return <RegistryContext.Provider value={registry}>{children}</RegistryContext.Provider>
}

const unsubscribed = () => () => {}
const noNatives: readonly NativeWidgetPlugin[] = []

/** The plugin registered under `id` now, following run-time registrations. */
export function useWidgetPlugin(id: string): WidgetPlugin | undefined {
  const registry = useContext(RegistryContext)
  const read = useCallback(() => registry?.plugin(id), [registry, id])
  return useSyncExternalStore(registry ? registry.subscribe : unsubscribed, read, read)
}

/** The native plugins, for what each draws beside a session (`SessionAccessory`). */
export function useNativePlugins(): readonly NativeWidgetPlugin[] {
  const registry = useContext(RegistryContext)
  if (!registry) return noNatives
  // Natives are fixed at composition; `natives()` is one array for the window's life.
  return registry.natives()
}
