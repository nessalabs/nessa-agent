/** Two equal samples do not settle a paused or delayed geometry animation. */
export function swipeGeometrySettled(previous, current) {
  return Boolean(
    previous &&
    !current.geometryActive &&
    current.offset !== 0 &&
    current.offset === previous.offset &&
    current.tray === previous.tray,
  )
}

/** Read the same row throughout its original three-second settlement budget. */
export async function settledOffset(wrapper, closed) {
  const deadline = Date.now() + 3_000
  let last = null
  for (;;) {
    const now = await wrapper.evaluate((element, from) => {
      const content = element.firstElementChild
      const tray = element.querySelector('[data-slot="workspace-swipe-row-actions"]')
      const geometry = ["width", "inlineSize", "transform", "translate", "left", "right"]
      const geometryActive = element.getAnimations({ subtree: true }).some((each) => {
        if (["finished", "idle"].includes(each.playState)) return false
        if (each.effect?.getComputedTiming().iterations === Infinity) return false
        return each.effect
          ?.getKeyframes()
          .some((frame) => geometry.some((property) => Object.hasOwn(frame, property)))
      })
      return {
        offset:
          content.getBoundingClientRect().left -
          element.getBoundingClientRect().left -
          from,
        tray: tray?.getBoundingClientRect().width ?? 0,
        geometryActive,
      }
    }, closed)
    if (swipeGeometrySettled(last, now)) return now
    if (Date.now() > deadline)
      throw new Error(
        `the open row never held still (${now.offset} px, tray ${now.tray} px)`,
      )
    last = now
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
}
