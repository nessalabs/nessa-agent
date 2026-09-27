import type * as React from "react"

/**
 * The window's icon vocabulary. It mirrors the icon contract in nessa_ui's
 * design-system contract ("Icon ownership", "React 19 public API"):
 *
 * | here                   | nessa_ui               |
 * | ---------------------- | ---------------------- |
 * | `DesktopIconProps`     | `NessaIconProps`       |
 * | `DesktopIconComponent` | `NessaIconComponent`   |
 * | `DesktopIconRole`      | `NessaIconRole`, plus the app's own roles |
 * | `DesktopIconOverrides` | `NessaIconOverrides`   |
 * | `DesktopIconProvider`  | `NessaIconProvider`    |
 * | `useDesktopIcon`       | `useNessaIcon`         |
 *
 * nessa_ui has not built that contract yet, so it lives here until it does;
 * then these become its types, with the app's roles as an extension.
 *
 * A family owns each role's artwork, viewbox, and stroke. The component that
 * shows an icon owns its size, colour (icons draw in `currentColor`),
 * animation, `aria-hidden`, and the accessible name of an icon-only control.
 */
export type DesktopIconProps = Omit<React.ComponentPropsWithRef<"svg">, "children"> & {
  size?: number | string
}

export type DesktopIconComponent = (props: DesktopIconProps) => React.ReactNode

/** nessa_ui's roles, by the names its contract gives them. */
const designSystemRoles = [
  "check",
  "close",
  "chevronDown",
  "chevronUp",
  "chevronLeft",
  "chevronRight",
  "moreHorizontal",
] as const

/** What the window's own chrome needs icons for, named by meaning rather than by picture. */
const desktopRoles = [
  // Window and panes
  "sidebar",
  "panelRight",
  "sessionList",
  "splitRight",
  "splitDown",
  "maximize",
  "restore",
  "back",
  "forward",
  "home",
  "search",
  // Sessions and channels
  "newSession",
  "add",
  "channel",
  "privateChannel",
  "needsYou",
  "running",
  // Composer
  "send",
  "attach",
  "folder",
  "folderAdd",
  "thinking",
  "fast",
  "access",
  "enter",
  // The home header's picture
  "customize",
  "nightScene",
  "chooseImage",
  "adjustImage",
  "zoomIn",
  "zoomOut",
  // What an agent did
  "file",
  "edit",
  "terminal",
  // Settings categories
  "preferences",
  "appearance",
  "workspace",
  "model",
  "connections",
  "privacy",
  "about",
] as const

export const desktopIconRoles = [...designSystemRoles, ...desktopRoles] as const

export type DesktopIconRole = (typeof desktopIconRoles)[number]

/** Some roles drawn differently; a provider merges these over what it inherits. */
export type DesktopIconOverrides = Partial<Record<DesktopIconRole, DesktopIconComponent>>

/** A whole family: every role drawn, so it can stand in as the built-in default. */
export type DesktopIconFamily = Record<DesktopIconRole, DesktopIconComponent>
