/**
 * A widget's body in a pane of its own or in the window: the plugin's view
 * for that place, or what ADR 326's table says in its stead
 * (`model/host-table.ts`). The chrome around it — title, way back, close —
 * is the workspace's; this draws only what the widget fills.
 *
 * The body is where the caret lands when its pane takes focus, or when what
 * held the caret inside it goes away (`data-widget-body`, ADR 326), for the
 * view to place further.
 */
import { useRef } from "react"
import { Button } from "@nessa-ui/react/button"
import { EmptyState } from "@nessa-ui/react/empty-state"
import { useHostContext } from "../adapters/dom/host-context"
import { hostDraws } from "../model/host-table"
import type { OpenPlace, WidgetAnswer } from "../model/widget-state"
import type { WidgetHost, WidgetPlugin } from "./plugin"
import { offeredBy } from "./widget-answer"
import "./widgets.css"

/** The attribute on a widget's body, where focus lands for it. */
export const widgetBodyAttribute = "data-widget-body"

export function WidgetBody({
  id,
  place,
  answer,
  plugin,
  host,
}: {
  id: string
  place: OpenPlace
  answer: WidgetAnswer
  plugin: WidgetPlugin | undefined
  host: WidgetHost
}) {
  const body = useRef<HTMLDivElement>(null)
  const context = useHostContext(body)
  const draws = hostDraws(place, answer, offeredBy(plugin))
  const View = plugin?.kind === "native" ? plugin.views[place] : undefined
  return (
    <div
      ref={body}
      className="widget-body"
      data-place={place}
      {...{ [widgetBodyAttribute]: "" }}
      tabIndex={-1}
    >
      {draws.kind === "view" && View ? (
        <View id={id} place={place} host={host} context={context} />
      ) : draws.kind === "waiting" ? (
        <p className="widget-waiting" role="status">
          {draws.name}
        </p>
      ) : draws.kind === "line" ? (
        <EmptyState
          variant="compact"
          className="widget-line"
          title={draws.text}
          action={
            draws.closes ? (
              <Button size="sm" variant="ghost" onClick={() => host.close()}>
                Close
              </Button>
            ) : undefined
          }
        />
      ) : null}
    </div>
  )
}
