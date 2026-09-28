import type { HostKind } from "../../../../host/features"
import { sidebarLimits } from "../../model/window-fit"
import { WorkspaceShell, type SidebarRegion } from "./workspace-shell"

/**
 * Three columns, as in Mail or Finder: a sidebar of channels, the chosen
 * channel's session list beside it, then the chat panes. Only the sidebar
 * region is this layout's; everything else is the shell's.
 */
const region: SidebarRegion = {
  layout: "columns",
  sidebar: { variant: "channels", defaultWidth: 240, limits: sidebarLimits },
  sessionList: true,
}

export function ThreeColumns(props: { hostKind: HostKind; browserSurface: boolean }) {
  return <WorkspaceShell region={region} {...props} />
}
