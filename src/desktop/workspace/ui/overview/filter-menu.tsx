import { DesktopIcon, type DesktopIconRole } from "../../../ui/icons"
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
  MenuShortcut,
} from "../../../ui/menu"
import { tooltip } from "../../../ui/tooltip"
import {
  choiceKey,
  filterLabel,
  rangeLabels,
  ranges,
  scopeLabels,
  scopes,
  sessionTags,
  type AgentsFilter,
  type AgentsRange,
  type AgentsScope,
} from "../../model/overview/filter"

const choiceIcons: Readonly<Record<AgentsScope | AgentsRange, DesktopIconRole>> = {
  ongoing: "ongoing",
  all: "sessionList",
  any: "anyTime",
  today: "today",
  week: "lastWeek",
  month: "lastMonth",
}

/**
 * What the overview lists, chosen beside its title: a quiet chip that names
 * the choice, opening the window's own menu — Ongoing or All, a span of time,
 * and tags, which sessions do not carry yet and the menu says so. Each
 * choice carries its number, and pressing it picks the choice and closes
 * the menu.
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
      <DropdownMenuContent
        align="start"
        sideOffset={6}
        onKeyDown={(event) => {
          if (event.metaKey || event.ctrlKey || event.altKey || !/^\d$/.test(event.key))
            return
          // Ahead of the menu's own type-to-find, which would read the digit.
          event.preventDefault()
          // The row the number names, chosen as a click chooses it: the same
          // pick, blink and close.
          event.currentTarget
            .querySelector<HTMLElement>(`[aria-keyshortcuts="${event.key}"]`)
            ?.click()
        }}
      >
        <MenuRadioGroup
          value={filter.scope}
          onValueChange={(scope) => {
            const chosen = scopes.find((each) => each === scope)
            if (chosen) onChange({ ...filter, scope: chosen })
          }}
        >
          {scopes.map((scope) => (
            <MenuRadioItem key={scope} value={scope} aria-keyshortcuts={choiceKey(scope)}>
              <DesktopIcon name={choiceIcons[scope]} />
              {scopeLabels[scope]}
              <MenuShortcut aria-hidden>{choiceKey(scope)}</MenuShortcut>
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
            <MenuRadioItem key={range} value={range} aria-keyshortcuts={choiceKey(range)}>
              <DesktopIcon name={choiceIcons[range]} />
              {rangeLabels[range]}
              <MenuShortcut aria-hidden>{choiceKey(range)}</MenuShortcut>
            </MenuRadioItem>
          ))}
        </MenuRadioGroup>
        <MenuSeparator />
        <MenuLabel>Tags</MenuLabel>
        {sessionTags.length === 0 ? (
          <MenuItem disabled>
            <DesktopIcon name="tag" />
            No tags yet
          </MenuItem>
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
              <DesktopIcon name="tag" />
              {tag.label}
            </MenuCheckboxItem>
          ))
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
