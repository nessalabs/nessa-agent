import { createContext, useContext, useMemo, type ReactNode } from "react"
import type {
  DesktopIconComponent,
  DesktopIconOverrides,
  DesktopIconProps,
  DesktopIconRole,
} from "./icon-contract"
import { mergeIconOverrides, resolveIcon } from "./icon-resolution"
import { nessaIcons } from "./nessa-icons"

/**
 * The provider and hook of the icon contract (see `icon-contract.ts` for how
 * each name maps to nessa_ui's). Resolution order, as the contract fixes it:
 *
 * ```text
 * <DesktopIcon icon={…}>          1. the component's own icon prop
 *   nearest DesktopIconProvider   2. its overrides
 *     parent providers …          3. theirs, merged beneath
 *       nessaIcons                4. the built-in family, in currentColor
 * ```
 *
 * The context holds only the merged overrides, memoised per provider, so a
 * provider re-renders its subtree only when what it resolves changes.
 */
const IconContext = createContext<DesktopIconOverrides>({})

export interface DesktopIconProviderProps {
  icons?: DesktopIconOverrides
  children: ReactNode
}

export function DesktopIconProvider({ icons, children }: DesktopIconProviderProps) {
  const parent = useContext(IconContext)
  const merged = useMemo(() => mergeIconOverrides(parent, icons), [parent, icons])
  return <IconContext.Provider value={merged}>{children}</IconContext.Provider>
}

/** The drawing for a role under the nearest provider; never undefined. */
export function useDesktopIcon(role: DesktopIconRole): DesktopIconComponent {
  const provided = useContext(IconContext)
  return resolveIcon(role, { provided, builtIn: nessaIcons })
}

/**
 * Draws a role. Decorative by default (`aria-hidden`, not focusable): the
 * control around it carries the name. Pass `aria-hidden={false}` and a label
 * for an icon that means something on its own. `icon` replaces the drawing
 * for this one place, ahead of any provider.
 */
export function DesktopIcon({
  name,
  icon,
  ...props
}: DesktopIconProps & { name: DesktopIconRole; icon?: DesktopIconComponent }) {
  const provided = useContext(IconContext)
  const Icon = resolveIcon(name, { local: icon, provided, builtIn: nessaIcons })
  return <Icon aria-hidden="true" focusable="false" {...props} />
}
