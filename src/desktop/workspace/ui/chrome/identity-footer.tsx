import { openSettings } from "../../../settings"
import { isMac } from "../../../adapters/platform"
import { useSideRailPreference } from "../../../adapters/window-preferences"
import { commandLabel } from "../../../model/keyboard"
import { IdentityButton, IdentityRow } from "../../../ui/identity"

/**
 * The sidebar's left-aligned foot: "nessa Studio", which opens Settings.
 * With the side rail offered, "nessa Studio" stands in the window's corner
 * beside the rail's toggle instead (`StudioCorner`), and the foot keeps only
 * its room and the mark slot the toggle stands in with the rail closed.
 */
export function IdentityFooter() {
  const [rail] = useSideRailPreference()
  return (
    <IdentityRow className="workspace-identity" mark={rail === "on" ? "none" : "studio"}>
      {rail === "on" ? (
        <span className="desktop-identity-room" aria-hidden="true" />
      ) : (
        <StudioButton />
      )}
    </IdentityRow>
  )
}

/**
 * "nessa Studio" beside the side rail's toggle, in the window's bottom-left
 * corner: one gap after the toggle's mark, open or closed, so toggling the
 * rail moves neither (`side-rail.css`).
 */
export function StudioCorner() {
  return (
    <div className="side-rail-studio">
      <StudioButton />
    </div>
  )
}

function StudioButton() {
  return (
    <IdentityButton
      product="Studio"
      label="nessa Studio Settings"
      shortcut={`${commandLabel(isMac)},`}
      onClick={() => openSettings()}
    />
  )
}
