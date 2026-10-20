/**
 * The workspace window, whichever layout it is in. Everything the two
 * layouts share lives here, once: the window's root and its light; the
 * titlebar's controls (sidebar toggle, Back, Forward); the keyboard
 * (`workspaceShortcuts`, one map); the quick switcher (⌘K, ⌘\); the pane grid
 * and everything in a pane; focus following the focused pane; drag and drop;
 * the folded sidebar's reveal from the edge; fitting the side columns to the
 * window; the Agents overview's layer over the list and the panes
 * (`ui/overview/`); Escape for the widget in front (`widget-escape.ts`). A layout (`three-columns.tsx`, `sessions-in-sidebar.tsx`)
 * says only how the sidebar region is composed — which sidebar, and whether a
 * session list stands beside it — as a `SidebarRegion`.
 *
 * This component coordinates. The orchestration is `useWorkspaceWindow`; the
 * markup is `WorkspaceLayout`.
 */
import type { HostKind } from "../../../../host/features"
import { useWorkspaceWindow } from "./use-workspace-window"
import { WorkspaceLayout, type SidebarRegion } from "./workspace-layout"

export type { SidebarRegion }

export function WorkspaceShell({
  region,
  hostKind,
  browserSurface,
}: {
  region: SidebarRegion
  hostKind: HostKind
  browserSurface: boolean
}) {
  return <WorkspaceLayout {...useWorkspaceWindow({ region, hostKind, browserSurface })} />
}
