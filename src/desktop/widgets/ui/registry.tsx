import type { ComponentType } from "react"
import {
  ExperimentCard,
  ExperimentSurface,
  useExperimentSession,
  useExperimentTitle,
} from "../../experiments"
import { SubagentsPanel, useSubagentTrail } from "../../subagents"
import type { WidgetRef } from "../model/widget"

/**
 * What a plugin draws: inline in the conversation, and opened — beside the
 * conversation or in a pane of its own (`placement`). `useTitle` names it for
 * a pane's header.
 */
export interface WidgetPlugin {
  /** What it is, for the header of a pane it fills: "Experiment". */
  readonly kind: string
  readonly Inline: ComponentType<{
    id: string
    opened: boolean
    /** `beside` keeps it in the conversation's pane; `pane` gives it its own. */
    onOpen: (where: "beside" | "pane") => void
  }>
  readonly Surface: ComponentType<{
    id: string
    placement: "beside" | "pane"
    wide?: boolean
    onToggleWide?: () => void
    onClose?: () => void
    /** Opens another widget in a pane of its own — a subagent from an experiment's swarm. */
    onOpenWidget?: (widget: WidgetRef) => void
  }>
  readonly useTitle: (id: string) => string | undefined
  /** The conversation the widget belongs to, if any: its pane links back to it. */
  readonly useSession: (id: string) => string | undefined
  /**
   * Where inside it the person is, when not at its top: the pane's breadcrumb
   * names it after the widget's kind, and the kind leads back out.
   */
  readonly useTrail?: (id: string) => WidgetTrail | null
}

export interface WidgetTrail {
  readonly label: string
  readonly onBack: () => void
}

const plugins: Record<string, WidgetPlugin> = {
  experiment: {
    kind: "Experiment",
    Inline: ExperimentCard,
    Surface: ExperimentSurface,
    useTitle: useExperimentTitle,
    useSession: useExperimentSession,
  },
  // A conversation's subagents, by the conversation's id. Never inline: it is
  // opened from the conversation's header, or from a subagent named elsewhere.
  subagents: {
    kind: "Subagents",
    Inline: () => null,
    Surface: SubagentsPanel,
    useTitle: () => "Subagents",
    useSession: (sessionId) => sessionId,
    useTrail: useSubagentTrail,
  },
}

/** The plugin that draws `widget`; undefined for one this window does not know. */
export function pluginFor(widget: WidgetRef): WidgetPlugin | undefined {
  return Object.hasOwn(plugins, widget.plugin) ? plugins[widget.plugin] : undefined
}
