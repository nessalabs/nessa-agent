import {
  Children,
  createContext,
  isValidElement,
  useContext,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type KeyboardEventHandler,
  type ReactElement,
  type ReactNode,
} from "react"
import { useControlLabel } from "@nessa-ui/react/lib/control-label"
import {
  SegmentedControl,
  SegmentedControlOption,
} from "@nessa-ui/react/segmented-control"
import { SettingsGroup, SettingsRow } from "@nessa-ui/react/settings-group"
import { Switch } from "@nessa-ui/react/switch"
import { namesItsTab, setting, type SettingId } from "../model/settings-catalogue"
import { useWorkspaceLayoutPreference } from "../../adapters/workspace-layout-preference"
import { type WorkspaceLayoutId, workspaceLayouts } from "../../model/workspace-layout"
import { tooltip } from "../../ui/tooltip"

/**
 * The pieces every Settings tab is built from, over the UI kit's settings
 * group and row (`@nessa-ui/react/settings-group`), its switch and its
 * segmented control. What is Settings' own is the wiring to the catalogue:
 * a row's name and description, whether it is available yet, the setting a
 * search just landed on, and saying once — not on every row — that a group
 * is not available yet.
 *
 * ```text
 *   Group ─────── a card of rows; says "Not available yet" once when none is
 *     │
 *     ├── Row ─── one catalogued setting: name, description and control
 *     └── ItemRow  something listed rather than catalogued (an agent, a key)
 *   SettingGroup  one catalogued setting that is a card of its own (theme, layout)
 * ```
 */

/** The setting a search just jumped to, which its row briefly shows. */
export const FoundSetting = createContext<SettingId | null>(null)

/**
 * What describes the controls of the enclosing row, as ids for their
 * `aria-describedby`: the row's description, and the note saying why its
 * controls rest, wherever that note is drawn.
 */
const RowDescription = createContext<string | undefined>(undefined)

/**
 * Whether the enclosing row's setting is available. One that is not
 * (`pending` in the catalogue) disables every control in its row.
 */
const RowAvailable = createContext(true)

/**
 * The id of the enclosing group's "Not available yet", where the group says
 * it once for all its rows; a row in it then does not say it again.
 */
const GroupNote = createContext<string | undefined>(undefined)

/** What a setting not available yet says: once per group, or on its own row. */
const notYet = "Not available yet"

/** The description ids a control takes, joined; none when there are none. */
function describedBy(...ids: (string | undefined)[]): string | undefined {
  const present = [...new Set(ids.filter(Boolean))]
  return present.length > 0 ? present.join(" ") : undefined
}

/**
 * The catalogued settings a group's rows show, read from its `Row`s. A group
 * whose rows are every one a catalogued setting not available yet says so
 * once, under its card.
 */
function rowsOf(children: ReactNode): SettingId[] | undefined {
  const elements = Children.toArray(children).filter(isValidElement)
  const rows = elements.filter(
    (element): element is ReactElement<{ id: SettingId }> => element.type === Row,
  )
  return rows.length > 0 && rows.length === elements.length
    ? rows.map((row) => row.props.id)
    : undefined
}

/** The footnote under a card: the note that it is not available yet, then the group's own. */
function footnoteOf(noteId: string | undefined, note: string | undefined): ReactNode {
  if (!noteId && !note) return undefined
  return (
    <>
      {noteId ? (
        <span id={noteId} className="settings-unavailable">
          {notYet}
        </span>
      ) : null}
      {noteId && note ? " · " : null}
      {note}
    </>
  )
}

/**
 * A card of rows. `title` only where a tab has more than one group; `note`
 * only where the rows alone would leave someone unsure (when it applies, or
 * where else it is set).
 */
export function Group({
  title,
  note,
  children,
}: {
  title?: string
  note?: string
  children: ReactNode
}) {
  const noteId = useId()
  const rows = rowsOf(children)
  const resting = rows?.every((id) => setting(id).pending === true) ?? false
  return (
    <SettingsGroup
      className="settings-group"
      title={title}
      data-pending={resting || undefined}
      footnote={footnoteOf(resting ? noteId : undefined, note)}
    >
      <GroupNote.Provider value={resting ? noteId : undefined}>
        {children}
      </GroupNote.Provider>
    </SettingsGroup>
  )
}

/**
 * A row for one catalogued setting: its name and description come from the
 * catalogue, and so does whether it is available yet, and in which layout
 * it applies — elsewhere its control is disabled and the row says where.
 */
/** Where a setting applies, said on its row in any other layout: "Three columns only", "Not in Classic". */
function appliesIn(layouts: readonly WorkspaceLayoutId[]): string {
  const label = (id: WorkspaceLayoutId) =>
    workspaceLayouts.find((each) => each.id === id)?.label ?? id
  const outside = workspaceLayouts.filter((each) => !layouts.includes(each.id))
  return outside.length === 1 && layouts.length > 1
    ? `Not in ${outside[0].label}`
    : `${layouts.map(label).join(" or ")} only`
}

export function Row({ id, children }: { id: SettingId; children?: ReactNode }) {
  const entry = setting(id)
  const found = useContext(FoundSetting) === id
  const groupNote = useContext(GroupNote)
  const [layout] = useWorkspaceLayoutPreference()
  const elsewhere =
    entry.layouts !== undefined && !entry.layouts.includes(layout)
      ? appliesIn(entry.layouts)
      : undefined
  const pending = entry.pending === true
  // Why its control rests, said on the row unless its group says it once.
  const reason =
    pending && groupNote === undefined
      ? notYet
      : elsewhere !== undefined
        ? elsewhere
        : undefined
  return (
    <ItemRow
      label={entry.label}
      detail={reason ?? entry.detail}
      setting={id}
      found={found}
      unavailable={pending || elsewhere !== undefined}
      describedBy={pending ? groupNote : undefined}
      control={children}
    />
  )
}

/**
 * A row: its name, a line under it where the name alone would leave someone
 * unsure, and its control at the end. Its control is named by the name and
 * described by the line (and by `describedBy`), so it is read as it is seen.
 * Used directly for something listed rather than catalogued, such as an
 * installed agent.
 */
export function ItemRow({
  label,
  detail,
  leading,
  control,
  children,
  setting: settingId,
  found,
  unavailable,
  describedBy: also,
  className,
  onKeyDown,
  ...data
}: {
  label: ReactNode
  detail?: ReactNode
  leading?: ReactNode
  control?: ReactNode
  /** Under the row's name, full width: a command, a confirmation, a list. */
  children?: ReactNode
  setting?: SettingId
  found?: boolean
  /** Not available: its controls rest, dimmed, and say why through `detail` or `describedBy`. */
  unavailable?: boolean
  /** The id of a note elsewhere that also describes this row's controls. */
  describedBy?: string
  className?: string
  /** Keys the row itself answers, such as Escape for a confirmation in it. */
  onKeyDown?: KeyboardEventHandler<HTMLDivElement>
} & { [data: `data-${string}`]: string | boolean | undefined }) {
  const detailId = useId()
  // A pending group's note, said once under its card, still describes this row.
  const inherited = useContext(RowDescription)
  const inGroup = useContext(RowAvailable)
  const available = inGroup && unavailable !== true
  return (
    <RowAvailable.Provider value={available}>
      <RowDescription.Provider
        value={describedBy(inherited, detail ? detailId : undefined, also)}
      >
        <SettingsRow
          {...data}
          onKeyDown={onKeyDown}
          className={className ? `settings-row ${className}` : "settings-row"}
          data-setting={settingId}
          // The kit's row owns its `data-found` and `data-disabled` (a row not
          // available yet is a disabled one): they are said through its props.
          found={found}
          label={label}
          detail={detail ? <span id={detailId}>{detail}</span> : undefined}
          leading={
            leading ? <span className="settings-row-leading">{leading}</span> : undefined
          }
          control={control ?? undefined}
          disabled={!available}
        >
          {children}
        </SettingsRow>
      </RowDescription.Provider>
    </RowAvailable.Provider>
  )
}

/**
 * A catalogued setting that is a whole card of its own, such as the theme
 * picker: the group takes the setting's name as its title — on screen unless
 * it only repeats its tab's — and says when it is not available yet.
 */
export function SettingGroup({
  id,
  note,
  pending,
  children,
}: {
  id: SettingId
  note?: string
  /**
   * Not available in this window, though the catalogue has it: what it
   * changes is not attached here (Integrations without a gateway).
   */
  pending?: boolean
  children: ReactNode
}) {
  const found = useContext(FoundSetting) === id
  const nameId = useId()
  const noteId = useId()
  const entry = setting(id)
  const available = entry.pending !== true && pending !== true
  return (
    <SettingsGroup
      className="settings-group"
      data-setting={id}
      data-found={found || undefined}
      data-pending={!available || undefined}
      data-title={namesItsTab(id) ? "hidden" : undefined}
      title={<span id={nameId}>{entry.label}</span>}
      footnote={footnoteOf(available ? undefined : noteId, note)}
    >
      <RowAvailable.Provider value={available}>
        <GroupNote.Provider value={available ? undefined : noteId}>
          <NamedBy id={nameId} describedBy={available ? undefined : noteId}>
            {children}
          </NamedBy>
        </GroupNote.Provider>
      </RowAvailable.Provider>
    </SettingsGroup>
  )
}

/** Names the controls inside by `id`, as a row names its own. */
const GroupName = createContext<string | undefined>(undefined)

function NamedBy({
  id,
  describedBy: note,
  children,
}: {
  id: string
  describedBy?: string
  children: ReactNode
}) {
  return (
    <GroupName.Provider value={id}>
      <RowDescription.Provider value={note}>{children}</RowDescription.Provider>
    </GroupName.Provider>
  )
}

/**
 * A radio group the arrow keys walk, as a radio group should: one Tab stop —
 * the chosen option — and ←/→ (↑/↓) choosing the one beside it, Home and End
 * the first and last. The kit has no card radio group yet (see `Choices`).
 */
function useRadioKeys<T extends string>(
  options: readonly { id: T }[],
  value: T,
  onChange: (next: T) => void,
) {
  const group = useRef<HTMLDivElement>(null)
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const at = options.findIndex((option) => option.id === value)
    const step =
      event.key === "ArrowRight" || event.key === "ArrowDown"
        ? 1
        : event.key === "ArrowLeft" || event.key === "ArrowUp"
          ? -1
          : 0
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? options.length - 1
          : step === 0
            ? -1
            : (at + step + options.length) % options.length
    const option = options[next]
    if (!option) return
    event.preventDefault()
    onChange(option.id)
    group.current?.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus()
  }
  return { ref: group, onKeyDown }
}

/**
 * Cards picked by sight — themes, icon families, layouts — named by their
 * group: a radio group, each card drawn by `art`. Settings' own until the
 * kit has a card radio group.
 */
export function Choices<T extends string>({
  options,
  value,
  onChange,
  art,
}: {
  options: readonly { id: T; label: string }[]
  value: T
  onChange: (next: T) => void
  art: (id: T) => ReactNode
}) {
  const name = useContext(GroupName)
  const description = useContext(RowDescription)
  const available = useContext(RowAvailable)
  const keys = useRadioKeys(options, value, onChange)
  return (
    <div
      className="settings-choices"
      role="radiogroup"
      aria-labelledby={name}
      aria-describedby={description}
      {...keys}
    >
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          role="radio"
          aria-checked={option.id === value}
          tabIndex={option.id === value ? 0 : -1}
          disabled={!available}
          className="settings-choice"
          onClick={() => onChange(option.id)}
        >
          {art(option.id)}
          <span>{option.label}</span>
        </button>
      ))}
    </div>
  )
}

/** An on/off setting: the kit's switch, named by its row and described by its row's line. */
export function Toggle({
  checked,
  onChange,
  label,
  disabled,
  "aria-labelledby": labelledBy,
}: {
  checked: boolean
  onChange: (next: boolean) => void
  /** Only outside a row; in one, the row's name is the switch's. */
  label?: string
  /** Resting for now, such as while a request it sent is in flight. */
  disabled?: boolean
  "aria-labelledby"?: string
}) {
  const description = useContext(RowDescription)
  const available = useContext(RowAvailable)
  return (
    <Switch
      className="settings-switch"
      checked={checked}
      onCheckedChange={onChange}
      disabled={!available || disabled === true}
      aria-label={label}
      aria-labelledby={label ? undefined : labelledBy}
      aria-describedby={label ? undefined : description}
    />
  )
}

/** One of a few choices, side by side: the kit's segmented control, named by its row. */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  "aria-labelledby": labelledBy,
}: {
  value: T
  options: readonly { id: T; label: string }[]
  onChange: (next: T) => void
  /** Only outside a row; in one, the row's name is the group's. */
  label?: string
  "aria-labelledby"?: string
}) {
  const name = useControlLabel({ "aria-label": label, "aria-labelledby": labelledBy })
  const description = useContext(RowDescription)
  const available = useContext(RowAvailable)
  return (
    <SegmentedControl
      variant="glass"
      className="settings-segmented"
      value={value}
      onValueChange={(next) => {
        const option = options.find((each) => each.id === next)
        if (option) onChange(option.id)
      }}
      {...name}
      aria-describedby={description}
    >
      {options.map((option) => (
        <SegmentedControlOption key={option.id} value={option.id} disabled={!available}>
          {option.label}
        </SegmentedControlOption>
      ))}
    </SegmentedControl>
  )
}

/**
 * A button for an action not available yet. It is shown so the page has its
 * shape, and disabled, saying so, so it does not pretend to have worked.
 */
export function PendingAction({ children }: { children: ReactNode }) {
  const description = useContext(RowDescription)
  return (
    <button
      type="button"
      className="settings-button"
      disabled
      aria-describedby={description}
      {...tooltip(notYet)}
    >
      {children}
    </button>
  )
}

/** Local, unsaved state for settings that only show their shape for now. */
export function usePrototype<T>(initial: T) {
  return useState<T>(initial)
}
