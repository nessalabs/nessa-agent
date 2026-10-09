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
import { useHostContext } from "../adapters/dom/host-context"
import { AppView } from "../app/ui/app-view"
import { hostDraws } from "../model/host-table"
import type { OpenPlace, WidgetAnswer } from "../model/widget-state"
import { readsHostSize, type WidgetHost, type WidgetPlugin } from "./plugin"
import { offeredBy } from "./widget-answer"
import { WidgetLine, WidgetWaiting } from "./widget-line"
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
  const draws = hostDraws(place, answer, offeredBy(plugin))
  const context = useHostContext(body, draws.kind === "view" && readsHostSize(plugin))
  const View = plugin?.kind === "native" ? plugin.views[place] : undefined
  return (
    <div
      ref={body}
      className="widget-body"
      data-place={place}
      {...{ [widgetBodyAttribute]: "" }}
      tabIndex={-1}
    >
      {draws.kind === "view" && plugin?.kind === "app" ? (
        <AppView plugin={plugin} id={id} place={place} host={host} context={context} />
      ) : draws.kind === "view" && View ? (
        <View id={id} place={place} host={host} context={context} />
      ) : draws.kind === "waiting" ? (
        <WidgetWaiting name={draws.name} />
      ) : draws.kind === "line" ? (
        <WidgetLine
          text={draws.text}
          closes={draws.closes}
          onClose={() => host.close()}
        />
      ) : null}
    </div>
  )
}
