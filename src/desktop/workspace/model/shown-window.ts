/**
 * What the window shows, as the person's agents are told it (`read_window`
 * on the gateway): the arrangement, which side columns are drawn, what fills
 * the content region, and the panes left to right, each column top to
 * bottom, each with its share of the room and what it shows. Built from the
 * window's own state (`application/usecases/window.ts`); the window is the
 * one authority for it, so the gateway keeps whatever it last said.
 *
 * Conversations are named by id only: their titles and agents are the
 * gateway's to say, read when an agent asks.
 */
import type { WidgetRef } from "../../widgets/model/widget-ref"

/** The layouts that draw the workspace's panes. */
export type WindowArrangement = "columns" | "sidebar"

/** What one pane shows. A new kind of pane (a file, say) is a new kind here. */
export type ShownItem =
  | { readonly kind: "conversation"; readonly sessionId: string }
  /** A new session the person has not started yet. */
  | { readonly kind: "draft" }
  | { readonly kind: "widget"; readonly widget: WidgetRef }

export interface ShownPane {
  /** Its share of its column's height, in whole percent. */
  readonly heightPercent: number
  readonly focused: boolean
  readonly item: ShownItem
}

export interface ShownColumn {
  /** Its share of the panes' width, in whole percent. */
  readonly widthPercent: number
  readonly panes: readonly ShownPane[]
}

export interface ShownWindow {
  readonly arrangement: WindowArrangement
  /** Whether the sidebar is drawn. */
  readonly sidebar: boolean
  /** Whether the session list is drawn; never in the sidebar arrangement, which has none. */
  readonly sessionList: boolean
  readonly showing:
    | { readonly kind: "panes" }
    | { readonly kind: "agents" }
    | { readonly kind: "widget"; readonly widget: WidgetRef }
  readonly columns: readonly ShownColumn[]
}

/** Shares relative to their siblings, as whole percents of their sum. */
export function percents(shares: readonly number[]): number[] {
  const total = shares.reduce((sum, share) => sum + share, 0)
  return shares.map((share) =>
    total > 0 ? Math.min(100, Math.max(0, Math.round((share / total) * 100))) : 0,
  )
}
