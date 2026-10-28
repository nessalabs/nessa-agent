import { durationToken } from "../../../adapters/motion"

/**
 * The controls that open a tool run, one call, or the calls folded above it.
 * A press on one of these holds the transcript's scroll
 * (`docs/design/transcript-tool-disclosure-scroll.md`).
 */
const disclosureControl =
  '[data-slot="tool-trace-line"], [data-slot="tool-trace-step"], [data-slot="tool-trace-earlier"]'

/**
 * Move to the end while the reader is there and no tool disclosure is holding
 * the scroll (`tool-disclosure.test.tsx`). A hold leaves `scrollTop` alone so
 * the clicked header stays and the details open downward.
 */
export function followTranscriptEnd(scroller: HTMLElement, following: boolean): void {
  if (!following || scroller.hasAttribute("data-disclosure-hold")) return
  scroller.scrollTop = scroller.scrollHeight
}

/**
 * While a tool disclosure is opening or closing, keep the scroller from
 * following its end and from anchoring to the text below the header.
 * `onRelease` runs once the motion has finished, without moving the scroll
 * (`tool-disclosure.test.tsx`).
 */
export function watchToolDisclosure(
  scroller: HTMLElement,
  onRelease: () => void,
): () => void {
  let frame = 0
  let timer = 0
  let holding = false
  let disposed = false
  const view = () => scroller.ownerDocument.defaultView ?? window
  const cancelPending = () => {
    const current = view()
    current.clearTimeout(timer)
    current.cancelAnimationFrame(frame)
    timer = 0
    frame = 0
  }
  const release = () => {
    if (disposed || !holding) return
    holding = false
    cancelPending()
    scroller.removeAttribute("data-disclosure-hold")
    onRelease()
  }
  const arm = () => {
    cancelPending()
    holding = true
    // Before the control's own handler opens it, so the layout that grows
    // the details already has anchoring off (`transcript.css`).
    scroller.setAttribute("data-disclosure-hold", "")
    const current = view()
    const motion = durationToken(scroller, "--nessa-motion-duration-normal")
    // One frame after the motion, so its last layout is still held.
    if (motion === 0)
      frame = current.requestAnimationFrame(() => {
        frame = current.requestAnimationFrame(release)
      })
    else
      timer = current.setTimeout(() => {
        frame = current.requestAnimationFrame(release)
      }, motion)
  }
  const onClick = (event: Event) => {
    if (disposed) return
    const target = event.target
    if (!(target instanceof Element)) return
    const control = target.closest(disclosureControl)
    if (!control || !scroller.contains(control)) return
    arm()
  }
  scroller.addEventListener("click", onClick, true)
  return () => {
    disposed = true
    scroller.removeEventListener("click", onClick, true)
    cancelPending()
    holding = false
    scroller.removeAttribute("data-disclosure-hold")
  }
}
