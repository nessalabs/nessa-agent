import type { HostKind } from "../../host/features"
import { useWorkspaceLayoutPreference } from "../adapters/workspace-layout-preference"
import { SettingsHost } from "../settings/ui/settings-view"
import { SessionsInSidebar, ThreeColumns } from "../workspace"
import { DesktopApp } from "./desktop-app"

/**
 * The window: the workspace in the layout chosen in Settings › Workspace ›
 * Layout — three columns, sessions in the sidebar, or the classic shell —
 * and Settings, which opens over whichever it is.
 */
export function DesktopWindow(props: { hostKind: HostKind; browserSurface: boolean }) {
  const [layout] = useWorkspaceLayoutPreference()
  return (
    <>
      {layout === "classic" ? (
        <DesktopApp {...props} />
      ) : layout === "sidebar" ? (
        <SessionsInSidebar {...props} />
      ) : (
        <ThreeColumns {...props} />
      )}
      <SettingsHost {...props} />
    </>
  )
}
