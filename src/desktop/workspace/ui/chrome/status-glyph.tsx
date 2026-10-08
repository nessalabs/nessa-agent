import type { SessionStatus } from "../../model/workspace-index"
import { statusLabels } from "../../model/session-groups"
import { tooltip } from "../../../ui/tooltip"

/** What a glyph can say: a session's state, or that a row holds something not yet seen. */
type GlyphKind = SessionStatus | "unread"

const glyphLabels: Readonly<Record<GlyphKind, string>> = {
  ...statusLabels,
  unread: "Unread",
}

/**
 * A session's state in one 10px box, so rows line up whatever it is: needs
 * you is a lit amber point, running a small turning arc, never a bare ring,
 * and unread a lit point in the running light. Idle shows nothing, or a
 * faint point where `idle` asks for one.
 *
 * `flush` draws a point at its own 6px, for a line of words that starts at
 * the point rather than at a column of glyphs. `decorative` says nothing to
 * a reader, beside words that already say it ("Needs you").
 */
export function StatusGlyph({
  status,
  idle = false,
  flush = false,
  decorative = false,
}: {
  status: GlyphKind
  idle?: boolean
  flush?: boolean
  decorative?: boolean
}) {
  if (status === "idle")
    return idle ? (
      <span className="workspace-status" data-status="idle" aria-hidden="true" />
    ) : null
  const says = decorative
    ? { "aria-hidden": true as const }
    : {
        role: "img",
        "aria-label": glyphLabels[status],
        ...tooltip(glyphLabels[status]),
      }
  return (
    <span
      className="workspace-status"
      data-status={status}
      data-flush={flush || undefined}
      {...says}
    />
  )
}
