import { DesktopIcon } from "../../ui/icons"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuLabel,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
  MenuShortcut,
} from "../../ui/menu"
import { summarizeArea, type Experiment } from "../model/experiment"
import { points } from "./format"
import { AreaMark } from "./parts"

const everyArea = "all"

/**
 * One area, or all of them, chosen from the window's own menu: a quiet chip
 * that names the choice, so a view's toolbar holds one control rather than a
 * row of every area. Each row says what its area has added to the best so far.
 */
export function AreaMenu({
  experiment,
  value,
  onChange,
  heading = "Show runs from",
  allLabel = "All areas",
}: {
  experiment: Experiment
  value: string | null
  onChange: (value: string | null) => void
  heading?: string
  allLabel?: string
}) {
  const chosen = experiment.areas.find((area) => area.id === value)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="xp-menu-chip"
          data-chosen={chosen ? true : undefined}
        >
          {chosen ? <AreaMark area={chosen} size={12} /> : null}
          <span>{chosen?.name ?? allLabel}</span>
          <DesktopIcon name="chevronDown" />
        </button>
      </DropdownMenuTrigger>
      {/* The menu portals out of the surface, so it carries the area hues itself. */}
      <DropdownMenuContent align="end" sideOffset={6} className="xp-menu">
        <MenuLabel>{heading}</MenuLabel>
        <MenuRadioGroup
          value={value ?? everyArea}
          onValueChange={(next) => onChange(next === everyArea ? null : next)}
        >
          <MenuRadioItem value={everyArea}>
            <span className="xp-menu-tile" data-all>
              <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden>
                <rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1.2" />
                <rect x="9" y="2.5" width="4.5" height="4.5" rx="1.2" />
                <rect x="2.5" y="9" width="4.5" height="4.5" rx="1.2" />
                <rect x="9" y="9" width="4.5" height="4.5" rx="1.2" />
              </svg>
            </span>
            {allLabel}
          </MenuRadioItem>
          <MenuSeparator />
          {experiment.areas.map((area) => {
            const gain = summarizeArea(experiment, area).gain
            return (
              <MenuRadioItem key={area.id} value={area.id}>
                <span
                  className="xp-menu-tile"
                  style={{ ["--xp-area" as string]: `var(--xp-area-${area.slot})` }}
                >
                  <AreaMark area={area} size={12} />
                </span>
                {area.name}
                <MenuShortcut>{gain > 0 ? points(gain) : "—"}</MenuShortcut>
              </MenuRadioItem>
            )
          })}
        </MenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
