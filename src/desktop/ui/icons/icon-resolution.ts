import {
  desktopIconRoles,
  type DesktopIconComponent,
  type DesktopIconFamily,
  type DesktopIconOverrides,
  type DesktopIconRole,
} from "./icon-contract"

/**
 * What a nested provider hands down: its parent's overrides with its own laid
 * over them, role by role. Roles it does not name keep the parent's drawing.
 * Returns the parent itself when there is nothing to add, so a provider with
 * no overrides does not change the context's identity.
 */
export function mergeIconOverrides(
  parent: DesktopIconOverrides,
  own: DesktopIconOverrides | undefined,
): DesktopIconOverrides {
  if (!own) return parent
  const merged: DesktopIconOverrides = { ...parent }
  // Only roles are copied: anything else a consumer's object carries is not an icon.
  for (const role of desktopIconRoles) {
    const icon = Object.hasOwn(own, role) ? own[role] : undefined
    // An explicit `undefined` inherits rather than erasing the parent's drawing.
    if (icon) merged[role] = icon
  }
  return merged
}

/**
 * The drawing for a role, in the contract's order: the component's own icon
 * prop, then the nearest provider, then its parents (already merged into
 * `provided`), then the built-in family. `provided` may come from anywhere a
 * consumer builds one, so it is read for what it owns.
 */
export function resolveIcon(
  role: DesktopIconRole,
  {
    local,
    provided,
    builtIn,
  }: {
    local?: DesktopIconComponent
    provided: DesktopIconOverrides
    builtIn: DesktopIconFamily
  },
): DesktopIconComponent {
  if (local) return local
  if (Object.hasOwn(provided, role)) {
    const icon = provided[role]
    if (icon) return icon
  }
  return builtIn[role]
}
