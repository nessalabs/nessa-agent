import { openSettings } from "../../../settings"
import { isMac } from "../../../adapters/platform"
import { commandLabel } from "../../../model/keyboard"
import { tooltip } from "../../../ui/tooltip"

/**
 * The sidebar's foot: "nessa Studio", which opens Settings. Its mark is the
 * side rail's toggle beside it — a separate control, one tab stop each.
 */
export function IdentityFooter() {
  return (
    <footer className="workspace-identity">
      <button
        type="button"
        className="desktop-identity-button workspace-identity-name"
        aria-label="nessa Studio Settings"
        {...tooltip("Settings", { shortcut: `${commandLabel(isMac)},` })}
        onClick={openSettings}
      >
        <b>nessa</b> <span>Studio</span>
      </button>
    </footer>
  )
}
