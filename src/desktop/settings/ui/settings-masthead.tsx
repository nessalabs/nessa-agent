import type { ReactNode } from "react"

/**
 * The head of a Settings page: the category's name, set large, with no line
 * under it; then the page's own way around it — its tabs — when it has
 * several.
 *
 * It scrolls with the page. Once its title has scrolled away, the page's bar
 * shows the name small (`SettingsView`), so a long page never loses it.
 */
export function SettingsMasthead({
  title,
  headingId,
  children,
}: {
  title: string
  headingId: string
  /** The page's tabs, under the title. */
  children?: ReactNode
}) {
  return (
    <header className="settings-masthead">
      <h1 id={headingId} className="settings-title">
        {title}
      </h1>
      {children}
    </header>
  )
}
