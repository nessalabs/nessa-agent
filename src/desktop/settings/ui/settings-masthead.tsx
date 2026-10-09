import type { ReactNode } from "react"

/**
 * The head of a Settings page: the category's name, set large; one short
 * line under it only where the name alone would leave someone unsure what
 * the page is for (`dekOf` in the catalogue); then the page's own way around
 * it — its tabs — when it has several.
 *
 * It scrolls with the page. Once its title has scrolled away, the page's bar
 * shows the name small (`SettingsView`), so a long page never loses it.
 */
export function SettingsMasthead({
  title,
  dek,
  headingId,
  children,
}: {
  title: string
  dek?: string
  headingId: string
  /** The page's tabs, under the title. */
  children?: ReactNode
}) {
  return (
    <header className="settings-masthead">
      <h1 id={headingId} className="settings-title">
        {title}
      </h1>
      {dek ? <p className="settings-dek">{dek}</p> : null}
      {children}
    </header>
  )
}
