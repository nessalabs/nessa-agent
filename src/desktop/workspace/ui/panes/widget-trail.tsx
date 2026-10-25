import { memo } from "react"
import { Breadcrumb, type BreadcrumbStep } from "@nessa-ui/react/breadcrumb"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectSession } from "../../adapters/store/selectors"
import type { WidgetCrumb } from "../../../widgets"

/**
 * A widget's name in its chrome, after the way back to the conversation it
 * belongs to (ADR 326): the conversation's title, which `onOrigin` goes back
 * to, then the widget's own. A widget with no origin, or whose origin is no
 * longer listed, has no way back: its name alone.
 */
export const WidgetTrail = memo(function WidgetTrail({
  origin,
  title,
  onOrigin,
  inner = null,
}: {
  origin: string | undefined
  title: string
  onOrigin: (sessionId: string) => void
  /** Where the view is inside itself (`WidgetHost.crumb`): the widget's name then steps back to its top. */
  inner?: WidgetCrumb | null
}) {
  const originTitle = useWorkspaceSelector((state) =>
    origin === undefined ? undefined : selectSession(state, origin)?.title,
  )
  const own: BreadcrumbStep[] = inner
    ? [{ label: title, onSelect: inner.back }, { label: inner.label }]
    : [{ label: title }]
  const steps: BreadcrumbStep[] =
    origin !== undefined && originTitle !== undefined
      ? [{ label: originTitle, onSelect: () => onOrigin(origin) }, ...own]
      : own
  return (
    <Breadcrumb
      className="workspace-widget-trail"
      label="Where this was opened from"
      items={steps}
    />
  )
})
