import type { ReactNode } from "react"
import { PageHeader } from "@nessa-ui/react/page-header"

/**
 * The head of a Settings page: the category's name in the kit's
 * `PageHeader`, as every page's, with no line under it; then the page's own
 * way around it — its tabs — when it has several.
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
    <div className="settings-masthead">
      <PageHeader
        title={title}
        titleProps={{ id: headingId, className: "settings-title" }}
      />
      {children}
    </div>
  )
}
