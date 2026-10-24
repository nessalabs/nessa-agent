import { waitUntilSettled } from "./browser.mjs"

/** Exact ink is evidence only while this same row is under the actual pointer. */
export function hoverFailures(sample, colors) {
  return [
    ...(!sample.connected ? ["the sampled row was detached"] : []),
    ...(!sample.hovered || !sample.pointerWithinRow
      ? ["the pointer left the sampled row"]
      : []),
    ...(sample.finiteActive ? ["the sampled row still has finite animation"] : []),
    ...(!colors.includes(sample.fill)
      ? [`its fill is ${sample.fill}, expected ${colors.join(" or ")}`]
      : []),
  ]
}

/** Pin the input/measurement identities, then use the shared finite-animation fence. */
export async function sampleHover(page, target, { frame, row, force } = {}) {
  await waitUntilSettled(page)
  const input = await target.elementHandle()
  if (!input) throw new Error("no hover target")
  const measured = frame
    ? (
        await input.evaluateHandle(
          (element, selectors) =>
            element.closest(selectors.frame)?.querySelector(selectors.row),
          { frame, row },
        )
      ).asElement()
    : input
  if (!measured) throw new Error("no row for the hover target")
  await page.evaluate(() => {
    const proof = { events: [] }
    proof.listen = (event) =>
      proof.events.push({ x: event.clientX, y: event.clientY, at: performance.now() })
    window.__nessaHoverSample = proof
    document.addEventListener("pointermove", proof.listen, true)
  })
  const read = () =>
    measured.evaluate((element) => {
      const events = window.__nessaHoverSample.events
      const pointer = events.at(-1)
      const under = pointer ? document.elementFromPoint(pointer.x, pointer.y) : null
      const animations = element.getAnimations({ subtree: true }).map((animation) => ({
        playState: animation.playState,
        currentTime: animation.currentTime,
        timing: animation.effect?.getComputedTiming(),
      }))
      return {
        fill: getComputedStyle(element).backgroundColor,
        connected: element.isConnected,
        hovered: element.matches(":hover"),
        pointerWithinRow: Boolean(under && element.contains(under)),
        pointer,
        animations,
        finiteActive: animations.filter(
          (animation) =>
            ["running", "pending"].includes(animation.playState) &&
            animation.timing?.iterations !== Infinity,
        ).length,
      }
    })
  try {
    await input.hover({ force })
    const afterInput = await read()
    await waitUntilSettled(page)
    const settled = await read()
    await page.mouse.move(1, 1)
    await waitUntilSettled(page)
    return { afterInput, settled, away: await read() }
  } finally {
    await page.evaluate(() => {
      document.removeEventListener("pointermove", window.__nessaHoverSample.listen, true)
      delete window.__nessaHoverSample
    })
    if (measured !== input) await measured.dispose()
    await input.dispose()
  }
}
