import type { HostKind } from "../../host/features"
import { useMotionInEffect } from "../adapters/motion-preference"
import { useThemeOnDocument } from "../adapters/theme-preference"
import { useWebviewMenu } from "../adapters/use-webview-menu"
import { useWindowTooltips } from "../adapters/use-window-tooltips"
import { useDriftInEffect } from "../adapters/window-preferences"
import { useWorkspaceLayoutPreference } from "../adapters/workspace-layout-preference"
import { SettingsHost, useSettingsOpening } from "../settings"
import { SessionsInSidebar, ThreeColumns } from "../workspace"
import { DesktopApp } from "./desktop-app"

/**
 * The window: the workspace in the layout chosen in Settings › Workspace ›
 * Layout — three columns, sessions in the sidebar, or the classic shell —
 * and Settings, which opens over whichever it is.
 *
 * `inspectable` is a development build: the webview's own right-click menu,
 * kept off the window otherwise, stays on ⌥-right-click for Inspect Element.
 */
export function DesktopWindow({
  inspectable,
  ...props
}: {
  hostKind: HostKind
  browserSurface: boolean
  inspectable: boolean
}) {
  const [layout] = useWorkspaceLayoutPreference()
  // One tooltip for the whole window, in its own glass.
  useWindowTooltips()
  // Right-clicks open the window's menus or none: the webview's only on text.
  useWebviewMenu({ inspectable })
  // The theme on the root too, for the menus and pickers that portal outside every surface.
  useThemeOnDocument()
  // The motion chosen in Settings, or the system's, on the root for everything that moves.
  useMotionInEffect()
  useDriftInEffect()
  const settings = useSettingsOpening()
  return (
    <>
      {/* Settings is modal: the window under it takes no focus or pointer while it is open. */}
      <div className="desktop-window-under" inert={settings.open || undefined}>
        {layout === "classic" ? (
          <DesktopApp {...props} />
        ) : layout === "sidebar" ? (
          <SessionsInSidebar {...props} />
        ) : (
          <ThreeColumns {...props} />
        )}
      </div>
      <SettingsHost {...props} open={settings.open} onClose={settings.close} />
    </>
  )
}
