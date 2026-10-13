import { CannotRun } from "./cli.mjs"
import { css, keys } from "./selectors.mjs"

/** Observe and act on the same mounted prefix, before a later render can finish it. */
export function pressMountedOverviewKeys(sel) {
  const column = document.querySelector(sel.column)
  if (!column) return false
  const rows = [...column.querySelectorAll(sel.item)]
  const focused = document.activeElement?.closest(sel.item)
  if (!focused || !rows.includes(focused)) return false
  if (column.hasAttribute("data-overview-listed")) return { missed: true }
  if (rows.length < 2 || focused === rows[rows.length - 1]) return false
  const ids = rows.map((row) => row.getAttribute("data-overview-item"))
  const press = (code) => {
    const target = document.activeElement
    if (!(target instanceof HTMLElement)) return
    target.dispatchEvent(
      new KeyboardEvent("keydown", {
        code,
        key: code,
        bubbles: true,
        cancelable: true,
      }),
    )
  }
  const now = () => {
    const row = document.activeElement?.closest(sel.item)
    const tabbable = rows.find((item) => item.tabIndex === 0)
    return {
      focused: row?.getAttribute("data-overview-item") ?? null,
      tabbable: tabbable?.getAttribute("data-overview-item") ?? null,
    }
  }
  const first = ids[0]
  const last = ids[ids.length - 1]
  press("End")
  const afterEnd = now()
  press("ArrowDown")
  const afterDown = now()
  press("Home")
  const afterHome = now()
  return { missed: false, first, last, afterEnd, afterDown, afterHome }
}

/** Arm before opening; the qualifying browser callback returns its own measurements. */
export async function keysWhileRowsArrive(page) {
  const pending = page.waitForFunction(
    pressMountedOverviewKeys,
    { column: css.overviewColumn, item: css.overviewItem },
    { timeout: 2000, polling: "raf" },
  )
  // A failed opening key must not leave an unobserved waiter rejection behind.
  void pending.catch(() => {})
  await page.keyboard.press(keys.overview)
  const handle = await pending
  let read
  try {
    read = await handle.jsonValue()
  } finally {
    await handle.dispose()
  }
  if (read.missed)
    throw new CannotRun("the overview completed without a qualifying partial list")
  const failures = []
  const aligned = (label, step, want) => {
    if (step.focused !== want)
      failures.push(`${label} left focus on ${step.focused}, not the mounted row ${want}`)
    else if (step.tabbable !== step.focused)
      failures.push(
        `${label} marked ${step.tabbable} current while focus stayed on ${step.focused}`,
      )
  }
  aligned("End", read.afterEnd, read.last)
  aligned("↓", read.afterDown, read.last)
  aligned("Home", read.afterHome, read.first)
  return { ...read, failures }
}
