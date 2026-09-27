import { Palette } from "lucide-react"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@nessa-ui/react/dropdown-menu"
import { desktopThemes, parseDesktopTheme, type DesktopThemeId } from "../model/theme"

/** Lists the themes, each with a swatch painted from its own colours. */
export function ThemeMenu({
  theme,
  onThemeChange,
}: {
  theme: DesktopThemeId
  onThemeChange: (theme: DesktopThemeId) => void
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="desktop-footer-button"
          aria-label="Appearance"
          title="Appearance"
        >
          <Palette aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="end" className="min-w-40">
        <DropdownMenuLabel>Light</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={theme}
          onValueChange={(value) => onThemeChange(parseDesktopTheme(value))}
        >
          {desktopThemes.map((option) => (
            <DropdownMenuRadioItem key={option.id} value={option.id}>
              <span
                aria-hidden="true"
                className="desktop-swatch"
                data-desktop-theme={option.id}
              />
              {option.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
