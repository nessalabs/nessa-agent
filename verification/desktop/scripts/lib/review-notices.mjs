/** Settings' notification row must agree with the browser-owned permission. */
export function reviewNoticeFailures({ permission, rows, allowButtons, text }) {
  if (permission === "unsupported")
    return rows === 0 ? [] : ["notification settings are shown without browser support"]

  const expected = {
    default: { allowButtons: 1, detail: "while you are away" },
    denied: { allowButtons: 0, detail: "Blocked in this browser." },
    granted: {
      allowButtons: 0,
      detail: "A review that is waiting shows a notification.",
    },
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
  if (!text.includes(state.detail))
    failures.push(`${permission}: the row does not explain its notification state`)
  return failures
}
