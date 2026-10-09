import { openSettings } from "../../../settings"
import { isMac } from "../../../adapters/platform"
import { commandLabel } from "../../../model/keyboard"
import { IdentityButton, IdentityRow } from "../../../ui/identity"

/**
 * The sidebar's foot: "nessa Studio", which opens Settings. Its mark is the
 * side rail's toggle beside it — a separate control, one tab stop each.
 */
export function IdentityFooter() {
  return (
    <IdentityRow className="workspace-identity">
      <IdentityButton
        product="Studio"
        label="nessa Studio Settings"
        shortcut={`${commandLabel(isMac)},`}
        onClick={openSettings}
      />
    </IdentityRow>
  )
}
