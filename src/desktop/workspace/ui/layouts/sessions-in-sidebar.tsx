import type { HostKind } from "../../../../host/features"
import { treeSidebarLimits } from "../../model/window-fit"
import { WorkspaceShell, type SidebarRegion } from "./workspace-shell"

/**
 * Sessions in the sidebar: one sidebar, where each channel discloses its
 * sessions inline, beside the chat panes — no session list. Its width, until
 * the person drags it, leaves room for a session's title and time. Only the
 * sidebar region is this layout's; everything else is the shell's.
 */
const region: SidebarRegion = {
  layout: "sidebar",
  sidebar: { variant: "tree", defaultWidth: 256, limits: treeSidebarLimits },
  sessionList: false,
}

export function SessionsInSidebar(props: {
  hostKind: HostKind
  browserSurface: boolean
}) {
  return <WorkspaceShell region={region} {...props} />
}
