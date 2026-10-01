/**
 * A transcript's widget part (ADR 326): a card in the message, drawn by the
 * plugin's inline view, or — ready, with none — a row with the widget's title
 * and Open, which asks the host for a pane (`open("pane")`). What it cannot
 * draw it says in one line, from the same table every host reads
 * (`model/host-table.ts`). A card has no close and no Escape of its own.
 */
import { useRef } from "react"
import { Button } from "@nessa-ui/react/button"
import { useHostContext } from "../adapters/dom/host-context"
import { hostDraws } from "../model/host-table"
import type { WidgetRef } from "../model/widget-ref"
import type { WidgetAnswer } from "../model/widget-state"
import type { WidgetHost, WidgetPlugin } from "./plugin"
import { offeredBy, WidgetAnswerOf } from "./widget-answer"
import "./widgets.css"

export function InlineWidget({ widget, host }: { widget: WidgetRef; host: WidgetHost }) {
  return (
    <WidgetAnswerOf widget={widget}>
      {(answer, plugin) => (
        <InlineCard id={widget.id} answer={answer} plugin={plugin} host={host} />
      )}
    </WidgetAnswerOf>
  )
}

function InlineCard({
  id,
  answer,
  plugin,
  host,
}: {
  id: string
  answer: WidgetAnswer
  plugin: WidgetPlugin | undefined
  host: WidgetHost
}) {
  const card = useRef<HTMLDivElement>(null)
  const context = useHostContext(card)
  const draws = hostDraws("inline", answer, offeredBy(plugin))
  const View = plugin?.kind === "native" ? plugin.views.inline : undefined
  return (
    <div ref={card} className="widget-inline" data-widget-inline={draws.kind}>
      {draws.kind === "view" && View ? (
        <View id={id} place="inline" host={host} context={context} />
      ) : draws.kind === "row" ? (
        <div className="widget-inline-row">
          <span className="widget-inline-title">{draws.title}</span>
          <Button size="sm" variant="ghost" onClick={() => host.open("pane")}>
            Open
          </Button>
        </div>
      ) : draws.kind === "waiting" ? (
        <p className="widget-waiting" role="status">
          {draws.name}
        </p>
      ) : draws.kind === "line" ? (
        <p className="widget-line-inline">{draws.text}</p>
      ) : null}
    </div>
  )
}
