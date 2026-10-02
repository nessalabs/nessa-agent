/**
 * What a host says in a widget's stead, drawn one way wherever it is said:
 * the quiet placeholder with the plugin's name while a widget is not read or
 * an app loads, and one line — in a card, plain; in a pane or the window,
 * with close — for what it cannot draw (`model/host-table.ts`,
 * `app/model/app-view.ts`).
 */
import { Button } from "@nessa-ui/react/button"
import { EmptyState } from "@nessa-ui/react/empty-state"
import type { WidgetPlace } from "../model/widget-state"

export function WidgetWaiting({ name, className }: { name: string; className?: string }) {
  return (
    <p
      className={className ? `widget-waiting ${className}` : "widget-waiting"}
      role="status"
    >
      {name}
    </p>
  )
}

export function WidgetLine({
  place,
  text,
  closes,
  onClose,
}: {
  place: WidgetPlace
  text: string
  closes: boolean
  onClose: () => void
}) {
  if (place === "inline") return <p className="widget-line-inline">{text}</p>
  return (
    <EmptyState
      variant="compact"
      className="widget-line"
      title={text}
      action={
        closes ? (
          <Button size="sm" variant="ghost" onClick={onClose}>
            Close
          </Button>
        ) : undefined
      }
    />
  )
}
