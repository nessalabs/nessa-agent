import { waitUntilSettled } from "./browser.mjs"

/** Exact ink is evidence only while the original input and control share a row. */
export function hoverFailures(sample, colors) {
  return [
    ...(!sample.connected ? ["the sampled row was detached"] : []),
    ...(!sample.inputWithinRow ? ["the hover input left its pinned row"] : []),
    ...(!sample.controlWithinRow ? ["the sampled control left its pinned row"] : []),
    ...(!sample.pointerWithinInput ? ["the pointer left the pinned hover input"] : []),
    ...(!sample.hovered || !sample.pointerWithinRow
      ? ["the pointer left the sampled row"]
      : []),
    ...(sample.finiteActive ? ["the sampled row still has finite animation"] : []),
    ...(!colors.includes(sample.fill)
      ? [`its fill is ${sample.fill}, expected ${colors.join(" or ")}`]
      : []),
  ]
}

/** Pin the input and row once. Fill, membership and hits share one DOM snapshot.
 * Sibling actions use the frame as pointer owner; their hits are captured before
 * the pointer leaves. Callers consume that evidence without selecting another row. */
export async function sampleHover(
  page,
  target,
  { frame, row, force, pointerOwner = "row", actions } = {},
) {
  if (!["row", "frame"].includes(pointerOwner) || (pointerOwner === "frame" && !frame))
    throw new Error("hover pointer owner must name the row or its configured frame")
  if (actions && pointerOwner !== "frame")
    throw new Error("sibling action hits require the configured frame as pointer owner")
  await waitUntilSettled(page)
  const input = await target.elementHandle()
  if (!input) throw new Error("no hover target")
  const handles = new Set([input])
  try {
    const containerHandle = frame
      ? await input.evaluateHandle(
          (element, selector) => element.closest(selector),
          frame,
        )
      : input
    handles.add(containerHandle)
    const container = containerHandle.asElement()
    if (!container) throw new Error("no frame for the hover target")
    const measuredHandle = frame
      ? await container.evaluateHandle(
          (element, selector) => element.querySelector(selector),
          row,
        )
      : input
    handles.add(measuredHandle)
    const measured = measuredHandle.asElement()
    if (!measured) throw new Error("no row for the hover target")
    const owner = pointerOwner === "frame" ? container : measured
    await page.evaluate(() => {
      const proof = { events: [] }
      proof.listen = (event) =>
        proof.events.push({ x: event.clientX, y: event.clientY, at: performance.now() })
      window.__nessaHoverSample = proof
      document.addEventListener("pointermove", proof.listen, true)
    })
    const read = () =>
      measured.evaluate(
        (element, { input, container, owner, actions, frame }) => {
          const pointer = window.__nessaHoverSample.events.at(-1)
          const under = pointer ? document.elementFromPoint(pointer.x, pointer.y) : null
          const box = input.getBoundingClientRect()
          // Compare with the captured row; never select another owner after input.
          const belongs = (node) =>
            Boolean(
              node &&
              (frame ? node.closest(frame) === container : container.contains(node)),
            )
          const action = actions
            ? [...container.querySelectorAll(actions)].find(belongs)
            : null
          const animations = owner.getAnimations({ subtree: true }).map((animation) => ({
            playState: animation.playState,
            currentTime: animation.currentTime,
            timing: animation.effect?.getComputedTiming(),
          }))
          return {
            fill: getComputedStyle(element).backgroundColor,
            connected: element.isConnected && container.isConnected && input.isConnected,
            inputWithinRow: belongs(input),
            controlWithinRow: belongs(element),
            pointerWithinInput: Boolean(
              pointer &&
              pointer.x >= box.left &&
              pointer.x <= box.right &&
              pointer.y >= box.top &&
              pointer.y <= box.bottom,
            ),
            hovered: owner.matches(":hover"),
            pointerWithinRow: Boolean(under && owner.contains(under) && belongs(under)),
            ...(actions
              ? {
                  hit: !belongs(under)
                    ? "elsewhere"
                    : action
                      ? action.contains(under)
                        ? "actions"
                        : "elsewhere"
                      : element.contains(under)
                        ? "row"
                        : "elsewhere",
                }
              : {}),
            pointer,
            animations,
            finiteActive: animations.filter(
              (animation) =>
                ["running", "pending"].includes(animation.playState) &&
                animation.timing?.iterations !== Infinity,
            ).length,
          }
        },
        { input, container, owner, actions, frame },
      )
    await input.hover({ force })
    const afterInput = await read()
    await waitUntilSettled(page)
    const settled = await read()
    await page.mouse.move(1, 1)
    await waitUntilSettled(page)
    return { pointerOwner, afterInput, settled, away: await read() }
  } finally {
    try {
      await page.evaluate(() => {
        if (window.__nessaHoverSample)
          document.removeEventListener(
            "pointermove",
            window.__nessaHoverSample.listen,
            true,
          )
        delete window.__nessaHoverSample
      })
    } finally {
      for (const handle of [...handles].reverse()) await handle.dispose()
    }
  }
}
