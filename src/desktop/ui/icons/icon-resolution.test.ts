import { describe, expect, it } from "vitest"
import {
  desktopIconRoles,
  type DesktopIconComponent,
  type DesktopIconFamily,
  type DesktopIconOverrides,
} from "./icon-contract"
import { mergeIconOverrides, resolveIcon } from "./icon-resolution"

const drawing = (name: string): DesktopIconComponent => {
  const icon: DesktopIconComponent = () => name
  return icon
}

const builtIn = Object.fromEntries(
  desktopIconRoles.map((role) => [role, drawing(`built-in ${role}`)]),
) as DesktopIconFamily

const local = drawing("local")
const nearest = drawing("nearest")
const parentIcon = drawing("parent")

describe("resolveIcon", () => {
  it("takes the component's own icon over every provider", () => {
    expect(resolveIcon("close", { local, provided: { close: nearest }, builtIn })).toBe(
      local,
    )
  })

  it("takes a provider's drawing over the built-in family", () => {
    expect(resolveIcon("close", { provided: { close: nearest }, builtIn })).toBe(nearest)
  })

  it("falls back to the built-in family for a role no provider draws", () => {
    expect(resolveIcon("search", { provided: { close: nearest }, builtIn })).toBe(
      builtIn.search,
    )
  })

  it("does not read what a provided object inherits", () => {
    const provided = Object.create({ close: nearest }) as Record<string, never>
    expect(resolveIcon("close", { provided, builtIn })).toBe(builtIn.close)
  })
})

describe("mergeIconOverrides", () => {
  it("lets the nearer provider's drawing win, role by role", () => {
    const merged = mergeIconOverrides(
      { close: parentIcon, search: parentIcon },
      { close: nearest },
    )
    expect(merged).toEqual({ close: nearest, search: parentIcon })
  })

  it("keeps the parent's drawing where the nearer provider says undefined", () => {
    expect(mergeIconOverrides({ close: parentIcon }, { close: undefined })).toEqual({
      close: parentIcon,
    })
  })

  it("returns the parent itself when there is nothing to add", () => {
    const parent = { close: parentIcon }
    expect(mergeIconOverrides(parent, undefined)).toBe(parent)
  })

  it("copies only roles, not whatever else an object carries", () => {
    const own: DesktopIconOverrides & { toString: DesktopIconComponent } = {
      close: nearest,
      toString: nearest,
    }
    expect(Object.keys(mergeIconOverrides({}, own))).toEqual(["close"])
  })

  it("leaves both inputs as they were", () => {
    const parent = { close: parentIcon }
    const own = { search: nearest }
    mergeIconOverrides(parent, own)
    expect(parent).toEqual({ close: parentIcon })
    expect(own).toEqual({ search: nearest })
  })
})
