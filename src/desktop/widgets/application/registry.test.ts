import { describe, expect, it, vi } from "vitest"
import { createWidgetRegistry, WidgetRegistryError } from "./registry"

type Plugin =
  | { readonly kind: "native"; readonly id: string; readonly name: string }
  | { readonly kind: "app"; readonly id: string; readonly name: string }

const native = (id: string, name = id): Extract<Plugin, { kind: "native" }> => ({
  kind: "native",
  id,
  name,
})
const app = (id: string, name = id): Extract<Plugin, { kind: "app" }> => ({
  kind: "app",
  id,
  name,
})

describe("composition", () => {
  it("looks each native plugin up by its id, and nothing else", () => {
    const registry = createWidgetRegistry<Plugin>([native("subagents"), native("sample")])
    expect(registry.plugin("subagents")?.id).toBe("subagents")
    expect(registry.plugin("sample")?.id).toBe("sample")
    expect(registry.plugin("missing")).toBeUndefined()
    // An inherited name is not a plugin.
    expect(registry.plugin("constructor")).toBeUndefined()
    expect(registry.natives().map((plugin) => plugin.id)).toEqual(["subagents", "sample"])
    expect(registry.natives()).toBe(registry.natives())
  })

  it("refuses two plugins under one id: the window does not start", () => {
    let thrown: unknown
    try {
      createWidgetRegistry<Plugin>([
        native("sample", "A"),
        native("x"),
        native("sample", "B"),
      ])
    } catch (error) {
      thrown = error
    }
    expect(thrown).toBeInstanceOf(WidgetRegistryError)
    expect((thrown as WidgetRegistryError).duplicates).toEqual(["sample"])
  })
})

describe("at run time", () => {
  it("registers an app plugin, and tells its listeners", () => {
    const registry = createWidgetRegistry<Plugin>([native("sample")])
    const heard = vi.fn()
    registry.subscribe(heard)
    expect(registry.register(app("mcp:rows"))).toEqual({ kind: "registered" })
    expect(registry.plugin("mcp:rows")?.kind).toBe("app")
    expect(heard).toHaveBeenCalledTimes(1)
  })

  it("refuses an app plugin under a native one's id, or another app's, changing nothing", () => {
    const registry = createWidgetRegistry<Plugin>([native("sample", "Native")])
    registry.register(app("mcp:rows", "First"))
    const heard = vi.fn()
    registry.subscribe(heard)
    expect(registry.register(app("sample"))).toEqual({
      kind: "refused",
      reason: "duplicate-id",
    })
    expect(registry.register(app("mcp:rows", "Second"))).toEqual({
      kind: "refused",
      reason: "duplicate-id",
    })
    expect(registry.plugin("sample")?.name).toBe("Native")
    expect(registry.plugin("mcp:rows")?.name).toBe("First")
    expect(heard).not.toHaveBeenCalled()
  })

  it("unregisters an app plugin, never a native one, and says so of an unknown id", () => {
    const registry = createWidgetRegistry<Plugin>([native("sample")])
    registry.register(app("mcp:rows"))
    const heard = vi.fn()
    const stop = registry.subscribe(heard)
    expect(registry.unregister("sample")).toEqual({ kind: "refused", reason: "native" })
    expect(registry.unregister("nothing")).toEqual({
      kind: "refused",
      reason: "unknown-id",
    })
    expect(heard).not.toHaveBeenCalled()
    expect(registry.unregister("mcp:rows")).toEqual({ kind: "unregistered" })
    expect(registry.plugin("mcp:rows")).toBeUndefined()
    expect(registry.plugin("sample")).toBeDefined()
    expect(heard).toHaveBeenCalledTimes(1)
    // Unregistered, an id may be registered again.
    stop()
    expect(registry.register(app("mcp:rows"))).toEqual({ kind: "registered" })
    expect(heard).toHaveBeenCalledTimes(1)
  })
})
