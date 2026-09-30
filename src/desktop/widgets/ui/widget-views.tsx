import { sameWidget, type WidgetRef } from "../model/widget"
import { pluginFor, type WidgetTrail } from "./registry"
import { useWidgetHost } from "./widget-host"
import "./widgets.css"

/** A widget where the conversation mentions it; a plugin this window lacks shows a quiet line. */
export function InlineWidget({ widget }: { widget: WidgetRef }) {
  const host = useWidgetHost()
  const inPane = host.useInPane(widget)
  const plugin = pluginFor(widget)
  if (!plugin)
    return <p className="widget-unknown">A {widget.plugin} this window cannot show.</p>
  const opened = sameWidget(host.opened, widget) || inPane
  return (
    <div className="widget-inline">
      <plugin.Inline
        id={widget.id}
        opened={opened}
        onOpen={(where) => (where === "pane" ? host.openPane(widget) : host.open(widget))}
      />
    </div>
  )
}

/** The pane's open widget, beside the conversation or over the whole pane. */
export function WidgetBeside({
  widget,
  wide,
  onToggleWide,
}: {
  widget: WidgetRef
  wide: boolean
  onToggleWide: () => void
}) {
  const host = useWidgetHost()
  const plugin = pluginFor(widget)
  if (!plugin) return null
  return (
    <div className="widget-beside" data-wide={wide || undefined}>
      <plugin.Surface
        id={widget.id}
        placement="beside"
        wide={wide}
        onToggleWide={onToggleWide}
        onClose={host.close}
        onOpenWidget={host.openPane}
      />
    </div>
  )
}

/** A widget filling a pane of its own; the pane's header closes it. */
export function WidgetInPane({
  widget,
  onOpenWidget,
}: {
  widget: WidgetRef
  onOpenWidget: (widget: WidgetRef) => void
}) {
  const plugin = pluginFor(widget)
  if (!plugin)
    return <p className="widget-unknown">A {widget.plugin} this window cannot show.</p>
  return (
    <div className="widget-in-pane">
      <plugin.Surface id={widget.id} placement="pane" onOpenWidget={onOpenWidget} />
    </div>
  )
}

/** A widget's name, for the header of the pane it fills. */
export function useWidgetTitle(widget: WidgetRef): string | undefined {
  const plugin = pluginFor(widget)
  // The registry is fixed for the window's life, so each pane asks the same hook every render.
  const useTitle = plugin?.useTitle ?? noTitle
  return useTitle(widget.id)
}

/** The conversation a widget belongs to, for its pane's link back. */
export function useWidgetSession(widget: WidgetRef): string | undefined {
  const plugin = pluginFor(widget)
  const useSession = plugin?.useSession ?? noTitle
  return useSession(widget.id)
}

/** Where inside a widget the person is, for its pane's breadcrumb; null at its top. */
export function useWidgetTrail(widget: WidgetRef): WidgetTrail | null {
  const useTrail = pluginFor(widget)?.useTrail ?? noTrail
  return useTrail(widget.id)
}

const noTrail = () => null

/** What a widget is, for its pane's header. */
export function widgetKind(widget: WidgetRef): string {
  return pluginFor(widget)?.kind ?? "Widget"
}

const noTitle = () => undefined
