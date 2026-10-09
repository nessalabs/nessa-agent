/**
 * The empty list's New Session and a failed workspace's Try Again (#657).
 * Both are the kit's tinted pill: 28px, fully round, the window's tint, and
 * a press that scales to 0.97. The kit's press writes the `scale` property
 * (`active:scale-[0.97]`), not `transform`, which is what the old
 * `.workspace-button` uses.
 */

/** Shape, tint, and height of one adopted pill. In the page. */
export function measureAdoptedPill([selector, name]) {
  const button = document.querySelector(selector)
  const failures = []
  if (!button) return { failures: [`${name} is missing`], height: 0 }
  if (button.classList.contains("workspace-button"))
    failures.push(`${name} is the window's old button`)
  const style = getComputedStyle(button)
  const box = button.getBoundingClientRect()
  if (Math.abs(box.height - 28) > 0.5)
    failures.push(`${name} is ${box.height}px tall, not 28`)
  const probe = document.createElement("div")
  probe.className = "rounded-full"
  document.querySelector("[data-surface]").append(probe)
  const pill = getComputedStyle(probe).borderTopLeftRadius
  probe.style.background = "var(--desktop-selected)"
  const tint = getComputedStyle(probe).backgroundColor
  probe.remove()
  if (style.borderTopLeftRadius !== pill)
    failures.push(
      `${name} corner ${style.borderTopLeftRadius}, not the kit's pill ${pill}`,
    )
  if (style.backgroundColor !== tint)
    failures.push(`${name} fill ${style.backgroundColor}, not the tint ${tint}`)
  return { failures, height: +box.height.toFixed(2) }
}

/**
 * Holds the pointer on `locator` and reads the kit's press scale.
 * The pointer is released off the button, so the press is not a click.
 */
export async function pressScale(page, locator, name) {
  const box = await locator.boundingBox()
  if (!box) return [`${name} is missing, so its press was not measured`]
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
  await page.mouse.down()
  const scale = await locator.evaluate((element) =>
    element.matches(":active") ? getComputedStyle(element).scale : "not-active",
  )
  await page.mouse.move(1, 1)
  await page.mouse.up()
  const value = Number.parseFloat(scale)
  if (!(Math.abs(value - 0.97) < 0.001)) return [`${name} press scale ${scale}, not 0.97`]
  return []
}
