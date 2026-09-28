import { DesktopIcon } from "../../../ui/icons"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuCheckboxItem,
  MenuItem,
  MenuLabel,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
} from "../../../ui/menu"
import { tooltip } from "../../../ui/tooltip"
import {
  filterLabel,
  rangeLabels,
  ranges,
  scopeLabels,
  scopes,
  sessionTags,
  type AgentsFilter,
} from "../../model/overview/filter"

/**
 * What the overview lists, chosen beside its title: a quiet chip that names
 * the choice, opening the window's own menu — Ongoing or All, a span of time,
 * and tags, which sessions do not carry yet and the menu says so.
 */
export function FilterMenu({
  filter,
  onChange,
}: {
  filter: AgentsFilter
  onChange: (filter: AgentsFilter) => void
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="agents-filter"
          aria-label={`Showing ${filterLabel(filter)}`}
          {...tooltip("Choose what’s listed")}
        >
          <span>{filterLabel(filter)}</span>
          <DesktopIcon name="chevronDown" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={6}>
        <MenuRadioGroup
          value={filter.scope}
          onValueChange={(scope) => {
            const chosen = scopes.find((each) => each === scope)
            if (chosen) onChange({ ...filter, scope: chosen })
          }}
        >
          {scopes.map((scope) => (
            <MenuRadioItem key={scope} value={scope}>
              {scopeLabels[scope]}
            </MenuRadioItem>
          ))}
        </MenuRadioGroup>
        <MenuSeparator />
        <MenuLabel>Updated</MenuLabel>
        <MenuRadioGroup
          value={filter.range}
          onValueChange={(range) => {
            const chosen = ranges.find((each) => each === range)
            if (chosen) onChange({ ...filter, range: chosen })
          }}
        >
          {ranges.map((range) => (
            <MenuRadioItem key={range} value={range}>
              {rangeLabels[range]}
            </MenuRadioItem>
          ))}
        </MenuRadioGroup>
        <MenuSeparator />
        <MenuLabel>Tags</MenuLabel>
        {sessionTags.length === 0 ? (
          <MenuItem disabled>No tags yet</MenuItem>
        ) : (
          sessionTags.map((tag) => (
            <MenuCheckboxItem
              key={tag.id}
              checked={filter.tags.includes(tag.id)}
              onCheckedChange={(on) =>
                onChange({
                  ...filter,
                  tags: on
                    ? [...filter.tags, tag.id]
                    : filter.tags.filter((each) => each !== tag.id),
                })
              }
            >
              {tag.label}
            </MenuCheckboxItem>
          ))
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
