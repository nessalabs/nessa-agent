/**
 * What the window knows of a widget (`WidgetAnswer`), resolved once per host
 * and handed to what draws it. Each plugin's hook is its own, so the answer
 * is read under a key per registered plugin: a plugin registered, replaced
 * or unregistered while a host is on the page mounts a fresh reader, and no
 * reader ever calls a different hook than the one it began with.
 */
import { useMemo, type ReactNode } from "react"
import { useWidgetPlugin } from "../adapters/react/registry-context"
import type { WidgetRef } from "../model/widget-ref"
import type { OfferedPlaces, WidgetAnswer } from "../model/widget-state"
import type { NativeWidgetPlugin, WidgetPlugin } from "./plugin"

type Draw = (answer: WidgetAnswer, plugin: WidgetPlugin | undefined) => ReactNode

const unregistered: WidgetAnswer = { registered: false }

// A key per plugin object, so a different registration under one id remounts.
const keys = new WeakMap<WidgetPlugin, number>()
let nextKey = 0
function keyOf(plugin: WidgetPlugin): number {
  let key = keys.get(plugin)
  if (key === undefined) keys.set(plugin, (key = nextKey++))
  return key
}

/** Reads `widget`'s answer from its plugin, and draws it with `children`. */
export function WidgetAnswerOf({
  widget,
  children,
}: {
  widget: WidgetRef
  children: Draw
}) {
  const plugin = useWidgetPlugin(widget.plugin)
  if (!plugin) return children(unregistered, undefined)
  if (plugin.kind === "app") return <AppAnswer plugin={plugin} draw={children} />
  return (
    <NativeAnswer key={keyOf(plugin)} plugin={plugin} id={widget.id} draw={children} />
  )
}

function NativeAnswer({
  plugin,
  id,
  draw,
}: {
  plugin: NativeWidgetPlugin
  id: string
  draw: Draw
}) {
  const state = plugin.useWidget(id)
  const answer = useMemo<WidgetAnswer>(
    () => ({ registered: true, name: plugin.name, state }),
    [plugin.name, state],
  )
  return draw(answer, plugin)
}

/** An MCP App's widget, which nothing draws until its renderer lands (#349). */
function AppAnswer({ plugin, draw }: { plugin: WidgetPlugin; draw: Draw }) {
  const answer = useMemo<WidgetAnswer>(
    () => ({ registered: true, name: plugin.name, state: { kind: "unshowable" } }),
    [plugin.name],
  )
  return draw(answer, plugin)
}

const noPlaces: OfferedPlaces = { inline: false, window: false }

/** The places beside the pane that a plugin offers a view for; none for one the window lacks. */
export function offeredBy(plugin: WidgetPlugin | undefined): OfferedPlaces {
  if (plugin?.kind !== "native") return noPlaces
  return { inline: Boolean(plugin.views.inline), window: Boolean(plugin.views.window) }
}
