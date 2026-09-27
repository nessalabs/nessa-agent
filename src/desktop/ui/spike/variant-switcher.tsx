import type { HostKind } from "../../../host/features"
import { DesktopApp } from "../desktop-app"
import { SettingsHost } from "../settings/settings-view"
import { VariantOne } from "./variant-one"
import { VariantTwo } from "./variant-two"
import { useWorkspaceLayout } from "./workspace-layout"

/** Spike only: shows the workspace layout chosen in the Appearance menu. */
export function VariantSwitcher(props: { hostKind: HostKind; browserSurface: boolean }) {
  const [layout] = useWorkspaceLayout()
  return (
    <>
      {layout === "sidebar" ? <VariantTwo {...props} /> : null}
      {layout === "classic" ? <DesktopApp {...props} /> : null}
      {layout === "columns" ? <VariantOne {...props} /> : null}
      <SettingsHost {...props} />
    </>
  )
}
