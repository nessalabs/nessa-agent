/**
 * A new session's life before its first message. It begins as a draft: a
 * pane shows its home, and nothing lists it. Its first message starts it —
 * titled from what was said — and only then does it join the lists. A draft
 * no pane shows any more is let go, leaving no trace. A session the source
 * never began, left with nothing to begin it when its last refused message
 * is let go, goes back to its home if a pane shows it, and is let go if not.
 *
 * ```text
 *   newSession ─▶ draft ──first message──▶ session (listed, revision 0)
 *                   │  ◀──last refused message discarded──┘ (shown; else gone)
 *                   └──no pane shows it──▶ gone
 * ```
 */
import type { ModelRef, SessionSummary } from "./workspace-index"
import { titleFrom } from "./transcript"

export interface Draft {
  /** The session's identity from the moment it began; its first message keeps it. */
  readonly id: string
  readonly channelId: string
  readonly model: ModelRef
}

/** The drafts some pane still shows; the same record when none was let go. */
export function keepShownDrafts(
  drafts: Readonly<Record<string, Draft>>,
  shown: ReadonlySet<string>,
): Readonly<Record<string, Draft>> {
  const ids = Object.keys(drafts)
  if (ids.every((id) => shown.has(id))) return drafts
  return Object.fromEntries(
    ids.filter((id) => shown.has(id)).map((id) => [id, drafts[id]]),
  )
}

/** The session a draft becomes with its first message: running, and titled by it. */
export function startSession(draft: Draft, text: string, at: number): SessionSummary {
  return {
    id: draft.id,
    channelId: draft.channelId,
    title: titleFrom(text),
    model: draft.model,
    status: "running",
    startedAt: at,
    updatedAt: at,
    preview: text,
    pinned: false,
    unread: false,
    // Nothing the source has said yet: its first word on the session supersedes this.
    revision: 0,
  }
}
