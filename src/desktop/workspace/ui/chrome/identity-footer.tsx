import { openSettings } from "../../../settings"
import { isMac } from "../../../adapters/platform"
import { commandLabel } from "../../../model/keyboard"
import { IdentityButton, IdentityRow } from "../../../ui/identity"

/**
 * The sidebar's left-aligned foot: "nessa Studio", which opens Settings.
 */
export function IdentityFooter() {
  return (
    <IdentityRow className="workspace-identity">
      <IdentityButton
        product="Studio"
        label="nessa Studio Settings"
        shortcut={`${commandLabel(isMac)},`}
        onClick={() => openSettings()}
      />
    </IdentityRow>
  )
}
