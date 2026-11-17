/** Settings' notification row must agree with the browser-owned permission. */
export function reviewNoticeFailures({ permission, rows, allowButtons, detail = "" }) {
  if (permission === "unsupported")
    return rows === 0 && allowButtons === 0 && detail === ""
      ? []
      : ["notification settings are shown without browser support"]

  const expected = {
    default: { allowButtons: 1, detail: "Off" },
    denied: {
      allowButtons: 0,
      detail:
        "Blocked in this browser. Allow notifications for this site to turn them on.",
    },
    granted: { allowButtons: 0, detail: "On" },
  }
  if (!Object.hasOwn(expected, permission))
    return [`unexpected notification permission ${permission}`]
  if (rows !== 1) return [`expected one review-notices row, found ${rows}`]
  const failures = []
  const state = expected[permission]
  if (allowButtons !== state.allowButtons)
    failures.push(
      `${permission}: expected ${state.allowButtons} accessible Allow buttons, found ${allowButtons}`,
    )
  if (detail !== state.detail)
    failures.push(`${permission}: the row does not explain its notification state`)
  return failures
}
