import type { SessionStatus } from "../../model/organisation"

const labels: Record<SessionStatus, string> = {
  running: "Working",
  "needs-you": "Needs you",
  idle: "",
}

/**
 * A session's state in one 10px box, so rows line up whatever it is: needs
 * you is a lit amber point, running a small turning arc, never a bare ring.
 * Idle shows nothing, or a faint point where `idle` asks for one.
 */
export function StatusGlyph({
  status,
  idle = false,
}: {
  status: SessionStatus
  idle?: boolean
}) {
  if (status === "idle")
    return idle ? (
      <span className="workspace-status" data-status="idle" aria-hidden="true" />
    ) : null
  return (
    <span
      className="workspace-status"
      data-status={status}
      role="img"
      aria-label={labels[status]}
      title={labels[status]}
    />
  )
}
