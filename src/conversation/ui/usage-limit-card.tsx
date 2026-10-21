import { useEffect, useState } from "react"
import {
  USAGE_LIMIT_TITLE,
  USAGE_LIMIT_TOGGLE,
  usageLimitDetail,
  type UsageLimitNotice,
} from "../model/usage-limit"
import "./usage-limit-card.css"

/**
 * The usage-limit card: the title, when the limit ends, and the gateway
 * preference. The checkbox shows the choice at once and keeps it until the
 * refreshed view agrees, the command fails, or the command has settled and
 * the confirmed preference is a different value. The line under the title
 * follows the confirmed preference, so a choice that has not landed does
 * not read as a scheduling failure.
 */
export function UsageLimitCard({
  limit,
  enabled,
  nowMs,
  onChange,
}: {
  limit: UsageLimitNotice
  /** The gateway preference. Absent is off. */
  enabled: boolean
  nowMs?: number
  onChange?: (enabled: boolean) => Promise<void> | void
}) {
  const [pending, setPending] = useState<boolean | null>(null)
  const [settled, setSettled] = useState(false)
  useEffect(() => {
    if (pending === null) return
    if (pending === enabled || settled) {
      setPending(null)
      setSettled(false)
    }
  }, [pending, enabled, settled])
  const shown = pending ?? enabled
  const detail = usageLimitDetail(limit, enabled, nowMs ?? Date.now())
  return (
    <section
      className="usage-limit-card"
      role="status"
      aria-label={USAGE_LIMIT_TITLE}
      data-usage-limit
      data-usage-limit-pending={pending === null ? undefined : ""}
    >
      <h3>
        <UsageLimitMark />
        {USAGE_LIMIT_TITLE}
      </h3>
      <p data-usage-limit-detail>{detail}</p>
      <label>
        <input
          type="checkbox"
          checked={shown}
          disabled={onChange === undefined}
          onChange={(event) => {
            if (!onChange || pending !== null) {
              event.preventDefault()
              return
            }
            const next = event.target.checked
            setPending(next)
            setSettled(false)
            void Promise.resolve(onChange(next)).then(
              () => setSettled(true),
              () => setPending(null),
            )
          }}
        />
        <span>{USAGE_LIMIT_TOGGLE}</span>
      </label>
    </section>
  )
}

function UsageLimitMark() {
  return (
    <svg className="usage-limit-card-mark" viewBox="0 0 16 16" aria-hidden="true">
      <path
        fill="currentColor"
        d="M8.9 1.7a1 1 0 0 0-1.8 0L1.2 12.4A1 1 0 0 0 2.1 14h11.8a1 1 0 0 0 .9-1.6L8.9 1.7zM8 5.2a.7.7 0 0 1 .7.7v3.2a.7.7 0 0 1-1.4 0V5.9A.7.7 0 0 1 8 5.2zm0 7a.8.8 0 1 1 0-1.6.8.8 0 0 1 0 1.6z"
      />
    </svg>
  )
}
