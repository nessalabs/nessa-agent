import { memo } from "react"
import { Breadcrumb, type BreadcrumbStep } from "@nessa-ui/react/breadcrumb"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectSession } from "../../adapters/store/selectors"

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
}: {
  origin: string | undefined
  title: string
  onOrigin: (sessionId: string) => void
}) {
  const originTitle = useWorkspaceSelector((state) =>
    origin === undefined ? undefined : selectSession(state, origin)?.title,
  )
  const steps: BreadcrumbStep[] =
    origin !== undefined && originTitle !== undefined
      ? [{ label: originTitle, onSelect: () => onOrigin(origin) }, { label: title }]
      : [{ label: title }]
  return (
    <Breadcrumb
      className="workspace-widget-trail"
      label="Where this was opened from"
      items={steps}
    />
  )
})
