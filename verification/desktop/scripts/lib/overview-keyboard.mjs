import { CannotRun, log } from "./cli.mjs"
import { css, keys } from "./selectors.mjs"

const observerAttribute = "data-overview-key-observer"
let nextObserverId = 0

/** Observe and act on the same mounted prefix, before a later render can finish it. */
export function pressMountedOverviewKeys(sel) {
  if (
    sel.observerId !== undefined &&
    document.documentElement?.getAttribute(sel.observerAttribute) !== sel.observerId
  )
    return false
  const column = document.querySelector(sel.column)
  if (!column) return false
  if (column.hasAttribute("data-overview-listed")) return { missed: true }
  const rows = [...column.querySelectorAll(sel.item)]
  const focused = document.activeElement?.closest(sel.item)
  if (!focused || !rows.includes(focused)) return false
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

/** A per-wait page token lets cleanup stop stale polls when the opening RPC settles. */
export async function keysWhileRowsArrive(page) {
  const observerId = String(++nextObserverId)
  let outcome
  let observerClosed = false
  let handle
  let handleDisposal
  const logCleanupFailure = (error) => {
    try {
      log("secondary overview keyboard cleanup fault:", error?.message ?? String(error))
    } catch {
      // A diagnostic failure must not replace the operation's primary outcome.
      return
    }
  }
  const disposeHandle = () => {
    if (!handle) return Promise.resolve()
    handleDisposal ??= Promise.resolve().then(() => handle.dispose())
    return handleDisposal
  }
  try {
    await page.evaluate(
      (id) => document.documentElement.setAttribute("data-overview-key-observer", id),
      observerId,
    )
    const pending = page
      .waitForFunction(
        pressMountedOverviewKeys,
        {
          column: css.overviewColumn,
          item: css.overviewItem,
          observerId,
          observerAttribute,
        },
        { timeout: 2000, polling: "raf" },
      )
      .then((observedHandle) => {
        handle = observedHandle
        if (!observerClosed) return observedHandle
        return disposeHandle()
          .catch((error) => {
            logCleanupFailure(error)
            throw error
          })
          .then(() => observedHandle)
      })
    // A failed opening key must not leave an unobserved waiter rejection behind.
    void pending.catch(() => {})
    await page.keyboard.press(keys.overview)
    handle = await pending
    const read = await handle.jsonValue()
    if (read.missed)
      throw new CannotRun("the overview completed without a qualifying partial list")
    const failures = []
    const aligned = (label, step, want) => {
      if (step.focused !== want)
        failures.push(
          `${label} left focus on ${step.focused}, not the mounted row ${want}`,
        )
      else if (step.tabbable !== step.focused)
        failures.push(
          `${label} marked ${step.tabbable} current while focus stayed on ${step.focused}`,
        )
    }
    aligned("End", read.afterEnd, read.last)
    aligned("↓", read.afterDown, read.last)
    aligned("Home", read.afterHome, read.first)
    outcome = { kind: "success", value: { ...read, failures } }
  } catch (error) {
    outcome = { kind: "failure", error }
  }
  observerClosed = true
  const cleanupFailures = []
  for (const cleanup of [
    () =>
      page.evaluate((id) => {
        const root = document.documentElement
        if (root?.getAttribute("data-overview-key-observer") === id)
          root.removeAttribute("data-overview-key-observer")
      }, observerId),
    disposeHandle,
  ]) {
    try {
      await cleanup()
    } catch (error) {
      cleanupFailures.push(error)
    }
  }
  if (outcome.kind === "failure") {
    for (const error of cleanupFailures) logCleanupFailure(error)
    throw outcome.error
  }
  if (cleanupFailures.length > 0) {
    for (const error of cleanupFailures.slice(1)) logCleanupFailure(error)
    throw cleanupFailures[0]
  }
  return outcome.value
}
