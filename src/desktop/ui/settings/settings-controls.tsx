import { createContext, useContext, useId, useState, type ReactNode } from "react"
import { setting, type SettingId } from "../../model/settings-catalogue"

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

/** A row for one catalogued setting: its name and description come from the catalogue. */
export function Row({ id, children }: { id: SettingId; children?: ReactNode }) {
  const entry = setting(id)
  const found = useContext(FoundSetting) === id
  return (
    <ItemRow
      label={entry.label}
      detail={entry.detail}
      setting={id}
      found={found}
      control={children}
    />
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
}: {
  label: string
  detail?: string
  leading?: ReactNode
  control?: ReactNode
  setting?: SettingId
  found?: boolean
}) {
  const nameId = useId()
  return (
    <div
      className="settings-row"
      data-setting={settingId}
      data-found={found || undefined}
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
 * picker: the group takes the setting's name as its title.
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
  return (
    <section className="settings-group" data-setting={id} data-found={found || undefined}>
      <h2 id={nameId}>{setting(id).label}</h2>
      <div className="settings-card">
        <RowName.Provider value={nameId}>{children}</RowName.Provider>
      </div>
      {footnote ? <p className="settings-footnote">{footnote}</p> : null}
    </section>
  )
}

/** Cards picked by sight — themes, icon families, layouts — named by their group. */
export function Choices({ children }: { children: ReactNode }) {
  const name = useControlName(undefined)
  return (
    <div className="settings-choices" role="radiogroup" {...name}>
      {children}
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
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
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
  return (
    <div className="settings-segmented" role="radiogroup" {...name}>
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          role="radio"
          aria-checked={option.id === value}
          onClick={() => onChange(option.id)}
        >
          {option.label}
        </button>
      ))}
    </div>
  )
}

/**
 * A button for an action this prototype cannot take yet. It is shown so the
 * page has its shape, and disabled so it does not pretend to have worked.
 */
export function PendingAction({ children }: { children: ReactNode }) {
  return (
    <button
      type="button"
      className="settings-button"
      disabled
      title="Not wired up in this prototype"
    >
      {children}
    </button>
  )
}

/** Local, unsaved state for settings that only show their shape for now. */
export function usePrototype<T>(initial: T) {
  return useState<T>(initial)
}
