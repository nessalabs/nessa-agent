import {
  createContext,
  useContext,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react"
import { setting, type SettingId } from "../model/settings-catalogue"
import { tooltip } from "../../ui/tooltip"

/**
 * The pieces every Settings tab is built from: grouped cards of rows, the
 * way System Settings lays them out, and the few quiet controls a row holds.
 */

/** The setting a search just jumped to, which its row briefly shows. */
export const FoundSetting = createContext<SettingId | null>(null)

/**
 * The id of the enclosing row's name, so a control inside a row is named by
 * the row's own text rather than by a second copy of it.
 */
const RowName = createContext<string | undefined>(undefined)

/**
 * Whether the enclosing row's setting is available. One that is not
 * (`pending` in the catalogue) disables every control in its row, which says
 * so beside its name.
 */
const RowAvailable = createContext(true)

/** What a row not available yet says, beside its name. */
const notYet = "Not available yet"

/** Names a control by its `label`, or else by the row it sits in. */
function useControlName(label: string | undefined) {
  const row = useContext(RowName)
  return label ? { "aria-label": label } : { "aria-labelledby": row }
}

export function Group({
  title,
  footnote,
  children,
}: {
  title?: string
  footnote?: string
  children: ReactNode
}) {
  return (
    <section className="settings-group">
      {title ? <h2>{title}</h2> : null}
      <div className="settings-card">{children}</div>
      {footnote ? <p className="settings-footnote">{footnote}</p> : null}
    </section>
  )
}

/**
 * A row for one catalogued setting: its name and description come from the
 * catalogue, and so does whether it is available yet.
 */
export function Row({ id, children }: { id: SettingId; children?: ReactNode }) {
  const entry = setting(id)
  const found = useContext(FoundSetting) === id
  const available = entry.pending !== true
  return (
    <RowAvailable.Provider value={available}>
      <ItemRow
        label={entry.label}
        detail={available ? entry.detail : notYet}
        setting={id}
        found={found}
        pending={!available}
        control={children}
      />
    </RowAvailable.Provider>
  )
}

/** A row for something listed rather than catalogued, such as an installed agent. */
export function ItemRow({
  label,
  detail,
  leading,
  control,
  setting: settingId,
  found,
  pending,
}: {
  label: string
  detail?: string
  leading?: ReactNode
  control?: ReactNode
  setting?: SettingId
  found?: boolean
  /** Not available yet: its detail says so, quietly. */
  pending?: boolean
}) {
  const nameId = useId()
  return (
    <div
      className="settings-row"
      data-setting={settingId}
      data-found={found || undefined}
      data-pending={pending || undefined}
    >
      {leading ? <span className="settings-row-leading">{leading}</span> : null}
      <div className="settings-row-text">
        <span id={nameId}>{label}</span>
        {detail ? <small>{detail}</small> : null}
      </div>
      {control ? (
        <div className="settings-row-control">
          <RowName.Provider value={nameId}>{control}</RowName.Provider>
        </div>
      ) : null}
    </div>
  )
}

/**
 * A catalogued setting that is a whole card of its own, such as the theme
 * picker: the group takes the setting's name as its title, and says when it
 * is not available yet.
 */
export function SettingGroup({
  id,
  footnote,
  children,
}: {
  id: SettingId
  footnote?: string
  children: ReactNode
}) {
  const found = useContext(FoundSetting) === id
  const nameId = useId()
  const entry = setting(id)
  const available = entry.pending !== true
  return (
    <section
      className="settings-group"
      data-setting={id}
      data-found={found || undefined}
      data-pending={!available || undefined}
    >
      <h2 id={nameId}>{entry.label}</h2>
      <div className="settings-card">
        <RowAvailable.Provider value={available}>
          <RowName.Provider value={nameId}>{children}</RowName.Provider>
        </RowAvailable.Provider>
      </div>
      {available ? null : <p className="settings-footnote">{notYet}</p>}
      {footnote ? <p className="settings-footnote">{footnote}</p> : null}
    </section>
  )
}

/**
 * A radio group the arrow keys walk, as a radio group should: one Tab stop —
 * the chosen option — and ←/→ (↑/↓) choosing the one beside it, Home and End
 * the first and last. Every choice in Settings is one of these.
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
 * group: a radio group, each card drawn by `art`.
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
  const name = useControlName(undefined)
  const available = useContext(RowAvailable)
  const keys = useRadioKeys(options, value, onChange)
  return (
    <div className="settings-choices" role="radiogroup" {...name} {...keys}>
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

export function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean
  onChange: (next: boolean) => void
  /** Only outside a row; in one, the row's name is the switch's. */
  label?: string
}) {
  const name = useControlName(label)
  const available = useContext(RowAvailable)
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={!available}
      {...name}
      className="settings-switch"
      onClick={() => onChange(!checked)}
    >
      <span />
    </button>
  )
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T
  options: readonly { id: T; label: string }[]
  onChange: (next: T) => void
  /** Only outside a row; in one, the row's name is the group's. */
  label?: string
}) {
  const name = useControlName(label)
  const available = useContext(RowAvailable)
  const keys = useRadioKeys(options, value, onChange)
  return (
    <div className="settings-segmented" role="radiogroup" {...name} {...keys}>
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          role="radio"
          aria-checked={option.id === value}
          tabIndex={option.id === value ? 0 : -1}
          disabled={!available}
          onClick={() => onChange(option.id)}
        >
          {option.label}
        </button>
      ))}
    </div>
  )
}

/**
 * A button for an action not available yet. It is shown so the page has its
 * shape, and disabled, saying so, so it does not pretend to have worked.
 */
export function PendingAction({ children }: { children: ReactNode }) {
  return (
    <button type="button" className="settings-button" disabled {...tooltip(notYet)}>
      {children}
    </button>
  )
}

/** Local, unsaved state for settings that only show their shape for now. */
export function usePrototype<T>(initial: T) {
  return useState<T>(initial)
}
