import { useThemePreference } from "../../../adapters/theme-preference"
import { openSettings } from "../../../settings/ui/settings-view"
import { ThemeMenu } from "../../../ui/theme-menu"
import { commandLabel } from "../../adapters/dom/shortcuts"

/** The sidebar's foot: "nessa Studio", which opens Settings, and the light theme's menu. */
export function IdentityFooter() {
  const [theme, setTheme] = useThemePreference()
  return (
    <footer className="workspace-identity">
      <span aria-hidden="true" className="desktop-mark" />
      <button
        type="button"
        className="desktop-identity-button workspace-identity-name"
        title={`Settings (${commandLabel},)`}
        onClick={openSettings}
      >
        <b>nessa</b> <span>Studio</span>
      </button>
      <ThemeMenu theme={theme} onThemeChange={setTheme} />
    </footer>
  )
}
