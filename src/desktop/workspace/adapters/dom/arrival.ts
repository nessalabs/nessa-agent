/** First-send motion: read the origin before send, observe the destination after layout. */
import { durationToken, motionToken } from "../../../adapters/motion"

/** Owned by the arrival adapter while its destinations await their first layout. */
export const arrivalWaitingAttribute = "data-message-arrival-waiting"

export interface Arrival {
  readonly composer: DOMRectReadOnly
  readonly text: DOMRectReadOnly
  readonly duration: number
  readonly glide: string
  readonly headingDuration: number
  readonly headingDelay: number
  readonly ease: string
}

/** Read the still-mounted home, before the command changes its DOM. */
export function captureArrival(home: HTMLElement | null): Arrival | null {
  const composer = home?.querySelector<HTMLElement>(".desktop-composer")
  const text = composer?.querySelector("textarea")
  if (!composer || !text) return null
  const duration = durationToken(composer, "--desktop-arrival")
  if (duration === 0) return null
  const composerBox = composer.getBoundingClientRect()
  const textBox = text.getBoundingClientRect()
  if (composerBox.width === 0 || textBox.height === 0) return null
  return {
    composer: composerBox,
    text: textBox,
    duration,
    glide: motionToken(composer, "--desktop-glide") ?? "linear",
    headingDuration: durationToken(composer, "--desktop-slow"),
    headingDelay: durationToken(composer, "--desktop-stagger"),
    ease: motionToken(composer, "--desktop-ease") ?? "linear",
  }
}

/**
 * One composer, first bubble and heading. The browser supplies all landing
 * boxes in one observation; starting the flights reads no layout. Cleanup
 * owns the observer, waiting mark and animations, including late deliveries.
 */
export function playArrival(
  conversation: HTMLElement,
  from: Arrival,
  done: () => void,
): () => void {
  const composer = conversation.querySelector<HTMLElement>(".desktop-composer")
  const bubble = conversation.querySelector<HTMLElement>(
    '[data-role="user"] .workspace-bubble',
  )
  const heading = conversation.querySelector<HTMLElement>(".workspace-heading")
  if (!composer || !bubble || !heading) {
    done()
    return () => {}
  }
  let active = true
  let launched = false
  const boxes = new Map<Element, DOMRectReadOnly>()
  const flights: Animation[] = []
  const observer = new IntersectionObserver((entries) => {
    if (!active || launched) return
    for (const entry of entries) boxes.set(entry.target, entry.boundingClientRect)
    const to = boxes.get(composer)
    const landed = boxes.get(bubble)
    if (!to || !landed || !boxes.has(heading)) return
    launched = true
    observer.disconnect()
    conversation.removeAttribute(arrivalWaitingAttribute)
    const scale = to.width > 0 ? from.composer.width / to.width : 1
    const dx = from.composer.left + from.composer.width / 2 - (to.left + to.width / 2)
    const dy = from.composer.top + from.composer.height / 2 - (to.top + to.height / 2)
    flights.push(
      composer.animate(
        [
          { transform: `translate(${dx}px, ${dy}px) scale(${scale})` },
          { transform: "none" },
        ],
        { duration: from.duration, easing: from.glide, id: "first-send-composer" },
      ),
      bubble.animate(
        [
          {
            transform: `translate(${from.text.left - landed.left - 16}px, ${from.text.top - landed.top - 8}px)`,
          },
          { transform: "none" },
        ],
        { duration: from.duration, easing: from.glide, id: "first-send-message" },
      ),
      heading.animate(
        [
          { opacity: 0, transform: "translateY(8px)" },
          { opacity: 1, transform: "none" },
        ],
        {
          duration: from.headingDuration,
          delay: from.headingDelay,
          easing: from.ease,
          fill: "backwards",
        },
      ),
    )
    Promise.all(flights.map((flight) => flight.finished))
      .then(() => {
        if (active) done()
      })
      .catch(() => undefined)
  })
  conversation.setAttribute(arrivalWaitingAttribute, "")
  for (const target of [composer, bubble, heading]) observer.observe(target)
  return () => {
    if (!active) return
    active = false
    observer.disconnect()
    conversation.removeAttribute(arrivalWaitingAttribute)
    flights.forEach((flight) => flight.cancel())
  }
}
