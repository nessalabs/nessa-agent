/**
 * A plugin of samples, registered by composition only while the sample
 * workspace is in use (`dependencies.ts`), so the hosts can be seen and
 * measured (`verification/desktop/scripts/widgets.mjs`) before any real
 * plugin lands. It holds nothing: what it answers is `sampleState`, and each
 * view keeps its own trail, as a plugin's places keep theirs (ADR 326).
 *
 * The trail is the case Escape and focus are built for: a step opened is a
 * detail the view registers a step back for (`onEscape`) while it is open;
 * Back, or Escape, closes it, and the caret that was in it falls back to the
 * widget's body.
 */
import { useEffect, useMemo, useState } from "react"
import { Button } from "@nessa-ui/react/button"
import type { WidgetState } from "../model/widget-state"
import type {
  NativeWidgetPlugin,
  SessionAccessoryProps,
  WidgetViewProps,
} from "../ui/plugin"
import { sampleState, sampleWidgets, samplePluginId } from "./sample-widgets"
import "./sample-plugin.css"

const steps = ["Read the case", "Try a change", "Measure it"] as const

/** A widget of the sample plugin's, in a pane or in the window. */
/** The place's size as the host reports it, for `widgets.mjs` to hold to the box it measures. */
function PlaceSize({ context }: { context: WidgetViewProps["context"] }) {
  const size = context.size
  return (
    <output
      className="sample-size"
      data-sample-size={size ? `${size.width}x${size.height}` : ""}
    >
      {size ? `${size.width} × ${size.height}` : "Not laid out"}
    </output>
  )
}

function SampleView({ id, place, host, context }: WidgetViewProps) {
  const [detail, setDetail] = useState<number | null>(null)
  // While a step is open, Escape steps back out of it, before the host's own.
  useEffect(() => {
    if (detail === null) return
    return host.onEscape(() => setDetail(null))
  }, [detail, host])
  if (id === sampleWidgets.notes.id)
    return (
      <section className="sample-view" data-sample-view={id}>
        <PlaceSize context={context} />
        <h2>Sample notes</h2>
        <p>Opened beside the conversation by the sample trail.</p>
      </section>
    )
  return (
    <section className="sample-view" data-sample-view={id}>
      <PlaceSize context={context} />
      {detail === null ? (
        <>
          <h2>Sample trail</h2>
          <ol className="sample-steps">
            {steps.map((step, index) => (
              <li key={step}>
                <Button
                  size="sm"
                  variant="ghost"
                  data-sample-step={index}
                  onClick={() => setDetail(index)}
                >
                  {step}
                </Button>
              </li>
            ))}
          </ol>
          <div className="sample-actions">
            {place === "pane" ? (
              <Button size="sm" variant="ghost" onClick={() => host.open("window")}>
                Open in Window
              </Button>
            ) : (
              <Button size="sm" variant="ghost" onClick={() => host.open("pane")}>
                Open in a Pane
              </Button>
            )}
            <Button
              size="sm"
              variant="ghost"
              onClick={() => host.openWidget(sampleWidgets.notes, "pane")}
            >
              Open Notes
            </Button>
          </div>
        </>
      ) : (
        <div className="sample-detail" data-sample-detail={detail}>
          <Button
            size="sm"
            variant="ghost"
            autoFocus
            data-sample-back
            onClick={() => setDetail(null)}
          >
            Back
          </Button>
          <h2>{steps[detail]}</h2>
          <p>A detail of the trail. Escape, or Back, steps out of it.</p>
        </div>
      )}
    </section>
  )
}

/** A sample widget's card in a message: its title, and the two places it opens in. */
function SampleCard({ id, host, context }: WidgetViewProps) {
  return (
    <div className="sample-card" data-sample-card>
      <PlaceSize context={context} />
      <span>{id === sampleWidgets.notes.id ? "Sample notes" : "Sample trail"}</span>
      <Button size="sm" variant="ghost" onClick={() => host.open("pane")}>
        Open
      </Button>
      <Button size="sm" variant="ghost" onClick={() => host.open("window")}>
        Open in Window
      </Button>
    </div>
  )
}

/** What the sample plugin draws in its own session's header: a way to its notes. */
function sampleAccessory(origin: string) {
  return function SampleAccessory({ sessionId, openWidget }: SessionAccessoryProps) {
    if (sessionId !== origin) return null
    return (
      <Button
        size="sm"
        variant="ghost"
        data-sample-accessory
        onClick={() => openWidget(sampleWidgets.notes, "pane")}
      >
        Notes
      </Button>
    )
  }
}

/** The sample plugin, its ready widgets belonging to the session `origin`. */
export function samplePlugin(origin: string): NativeWidgetPlugin {
  return {
    kind: "native",
    id: samplePluginId,
    name: "Sample",
    useWidget: function useSampleWidget(id: string): WidgetState {
      return useMemo(() => sampleState(id, origin), [id])
    },
    views: { pane: SampleView, window: SampleView, inline: SampleCard },
    readsHostSize: true,
    SessionAccessory: sampleAccessory(origin),
  }
}
