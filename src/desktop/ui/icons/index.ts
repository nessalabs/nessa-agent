/**
 * The window's icons: semantic roles resolved through providers to a family's
 * drawings. A mirror of nessa_ui's planned icon contract, kept here until the
 * design system ships it (see `icon-contract.ts`).
 *
 * ```text
 * icon-families ──▶ icon-provider ──▶ icon-resolution ──▶ icon-contract
 *       │                 │
 *       ▼                 ▼
 *  lucide-icons      nessa-icons (the built-in default)
 * ```
 *
 * An arrow points from a module to one it depends on. `icon-families` maps
 * each family id to its drawings and mounts the root provider from the
 * remembered choice.
 */
export {
  desktopIconRoles,
  type DesktopIconComponent,
  type DesktopIconFamily,
  type DesktopIconOverrides,
  type DesktopIconProps,
  type DesktopIconRole,
} from "./icon-contract"
export {
  DesktopIcon,
  DesktopIconProvider,
  useDesktopIcon,
  type DesktopIconProviderProps,
} from "./icon-provider"
export { DesktopIconFamilyProvider, iconFamilyDrawings } from "./icon-families"
