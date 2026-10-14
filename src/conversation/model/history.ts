/**
 * What a bounded conversation view says about history it did not draw.
 *
 * The gateway owns both facts (`truncated`, `transcriptState`). This names
 * them once, for every surface that shows the view: the panel's controls and
 * the desktop transcript. A surface does not fold committed records to fill
 * in what the view left out.
 */
import type { ConversationTranscriptState } from "./types"

/**
 * `omitted` is older text the view left out. `unconfirmed` is a transcript
 * the gateway has not finished reading. Either can be set alone.
 */
export interface HistoryExtent {
  readonly omitted: boolean
  readonly unconfirmed: boolean
}

/** One sentence a surface shows above the messages the view did draw. */
export interface HistoryNotice {
  readonly kind: "omitted" | "unconfirmed"
  readonly text: string
}

/** The sentence for history the view left out. */
export const omittedHistoryNotice = "Some conversation content is not shown."

/** The sentence for history the gateway has not confirmed. */
export const unconfirmedHistoryNotice =
  "Conversation history is not confirmed. Recent activity may be missing."

/**
 * Whether a transcript state is the gateway's word that history is confirmed.
 * The mapped type is the enforcer: a member added to
 * `ConversationTranscriptState` does not compile until this table says which.
 */
const historyConfirmed: { readonly [K in ConversationTranscriptState]: boolean } = {
  not_loaded: false,
  partial: false,
  complete_empty: true,
  complete: true,
  stale: false,
  unknown: false,
}

/**
 * What the view says it left out. Nothing when there is no view, or when it
 * says this is the whole confirmed history.
 */
export function historyExtent(
  view:
    | {
        readonly truncated: boolean
        readonly transcriptState: ConversationTranscriptState
      }
    | undefined,
): HistoryExtent | undefined {
  if (!view) return undefined
  const omitted = view.truncated
  const unconfirmed = !historyConfirmed[view.transcriptState]
  return omitted || unconfirmed ? { omitted, unconfirmed } : undefined
}

/** The notices `history` asks a surface to show, omitted first. */
export function historyNotices(
  history: HistoryExtent | undefined,
): readonly HistoryNotice[] {
  if (!history) return []
  return [
    ...(history.omitted
      ? [{ kind: "omitted" as const, text: omittedHistoryNotice }]
      : []),
    ...(history.unconfirmed
      ? [{ kind: "unconfirmed" as const, text: unconfirmedHistoryNotice }]
      : []),
  ]
}
