import { IconButton } from "./icon-button"
import { desktopThemes, parseDesktopTheme, type DesktopThemeId } from "../model/theme"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuLabel,
  MenuRadioGroup,
  MenuRadioItem,
} from "./menu"

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
        <IconButton
          icon="appearance"
          label="Appearance"
          size="sm"
          tone="faint"
          tooltipSide="above"
        />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="end">
        <MenuLabel>Light</MenuLabel>
        <MenuRadioGroup
          value={theme}
          onValueChange={(value) => onThemeChange(parseDesktopTheme(value))}
        >
          {desktopThemes.map((option) => (
            <MenuRadioItem key={option.id} value={option.id}>
              <span
                aria-hidden="true"
                className="desktop-swatch"
                data-desktop-theme={option.id}
              />
              {option.label}
            </MenuRadioItem>
          ))}
        </MenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
