/**
 * An MCP App's view in a place (ADR 344, #349): the sandbox proxy's frame,
 * spoken to through the bridge, and what the host says around it — the
 * plugin's name while it loads, one line when it cannot be shown, and notices
 * above it while it runs (`model/app-view.ts`).
 *
 * One bridge per view, for the life of the view: a call or a context that
 * changes is handed to it; a place that goes takes the frame, the bridge and
 * its listener with it (`remove`), and nothing is posted after.
 */
import { useCallback, useEffect, useRef, useState } from "react"
import type { HostContext, WidgetPlace } from "../../model/widget-state"
import type { AppWidgetPlugin, WidgetHost } from "../../ui/plugin"
import { WidgetLine, WidgetWaiting } from "../../ui/widget-line"
import { frameTransport, type FrameTransport } from "../adapters/dom/frame-transport"
import { createAppBridge, type AppBridge } from "../application/bridge"
import { appDraws, firstView } from "../model/app-view"
import type { AppCall } from "../model/tool-call"
import { useAppCall } from "./use-app-call"
import "./app-view.css"

/** The attribute on an app's proxy frame; its value is the place it is drawn in. */
export const appFrameAttribute = "data-app-frame"

/** The height an inline app is drawn at until it says how tall it is. */
const inlineHeight = 160

export function AppView({
  plugin,
  id,
  place,
  host,
  context,
}: {
  plugin: AppWidgetPlugin
  id: string
  place: WidgetPlace
  host: WidgetHost
  context: HostContext
}) {
  const read = useAppCall(plugin, id)
  // The answer said ready, so the call is known; it can go between the two reads.
  if (read.kind !== "known") return null
  return (
    <AppFrame
      plugin={plugin}
      call={read.call}
      place={place}
      host={host}
      context={context}
    />
  )
}

function AppFrame({
  plugin,
  call,
  place,
  host,
  context,
}: {
  plugin: AppWidgetPlugin
  call: AppCall
  place: WidgetPlace
  host: WidgetHost
  context: HostContext
}) {
  const [view, setView] = useState(firstView)
  const bridge = useRef<AppBridge | undefined>(undefined)
  const transport = useRef<FrameTransport | undefined>(undefined)
  // What this render was given, for a bridge that outlives it: kept first,
  // before the effects below read it (effects run in the order written).
  const latest = useRef({ host, call, context })
  useEffect(() => {
    latest.current = { host, call, context }
  })
  const { sandbox } = plugin.ports

  useEffect(() => {
    const created = createAppBridge({
      place,
      server: plugin.server,
      call: latest.current.call,
      context: latest.current.context,
      ports: plugin.ports,
      post: (message) => transport.current?.post(message),
      host: {
        open: (to) => latest.current.host.open(to),
        close: () => latest.current.host.close(),
      },
      onView: setView,
    })
    bridge.current = created
    return () => {
      created.remove()
      if (bridge.current === created) bridge.current = undefined
    }
  }, [plugin, place, call.sessionId, call.resourceUri])

  useEffect(() => bridge.current?.setCall(call), [call])
  useEffect(() => bridge.current?.setContext(context), [context])

  const attach = useCallback(
    (frame: HTMLIFrameElement | null) => {
      transport.current?.close()
      transport.current = undefined
      if (frame && sandbox)
        transport.current = frameTransport(frame, sandbox.origin, (message) =>
          bridge.current?.receive(message),
        )
    },
    [sandbox],
  )

  const draws = appDraws(place, view)
  return (
    <div className="widget-app" data-app-view={view.lifecycle.kind} data-place={place}>
      {draws.notices.map((notice) => (
        <p key={notice} className="widget-app-notice" role="status">
          {notice}
        </p>
      ))}
      {draws.frame !== "none" && sandbox ? (
        <iframe
          ref={attach}
          className="widget-app-frame"
          src={sandbox.url}
          sandbox="allow-scripts allow-same-origin"
          referrerPolicy="no-referrer"
          title={`${call.tool}, from ${plugin.name}`}
          data-hidden={draws.frame === "hidden" ? "" : undefined}
          style={place === "inline" ? { height: view.height ?? inlineHeight } : undefined}
          {...{ [appFrameAttribute]: place }}
        />
      ) : null}
      {draws.waiting ? (
        <WidgetWaiting name={plugin.name} className="widget-app-waiting" />
      ) : null}
      {draws.line ? (
        <WidgetLine
          text={draws.line.text}
          closes={draws.line.closes}
          onClose={() => host.close()}
        />
      ) : null}
    </div>
  )
}
