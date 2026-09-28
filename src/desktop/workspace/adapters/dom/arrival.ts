/**
 * A new session's first message, sent from its home: the composer glides
 * from the home down to the foot of the pane, the message rises from where it
 * was typed into its bubble, and the heading settles in above it. Transform
 * and opacity only. The home is measured before the conversation replaces
 * it; the motion plays once the conversation has mounted.
 */
import { useLayoutEffect, type RefObject } from "react"
import { durationToken, motionToken } from "../../../adapters/motion"

/** Where the first message was when it was sent. */
export interface Arrival {
  readonly composer: DOMRect
  readonly text: DOMRect
}

/** Measures a home's composer before it hands over; nothing when there is no motion to play. */
export function measureArrival(home: HTMLElement | null): Arrival | null {
  const composer = home?.querySelector(".desktop-composer")
  const field = composer?.querySelector("textarea")
  if (!composer || !field || durationToken(composer, "--desktop-arrival") === 0)
    return null
  return {
    composer: composer.getBoundingClientRect(),
    text: field.getBoundingClientRect(),
  }
}

/**
 * Plays the arrival once, on mount, from `arrival` into the conversation's
 * docked composer, first message and heading.
 */
export function useArrival(
  arrival: Arrival | null,
  {
    dock,
    scroller,
    heading,
  }: {
    dock: RefObject<HTMLElement | null>
    scroller: RefObject<HTMLElement | null>
    heading: RefObject<HTMLElement | null>
  },
): void {
  useLayoutEffect(() => {
    const from = arrival
    const composer = dock.current?.querySelector<HTMLElement>(".desktop-composer")
    const bubble = scroller.current?.querySelector<HTMLElement>(
      '[data-role="user"] .workspace-bubble',
    )
    const title = heading.current
    if (!from || !composer || !bubble || !title) return
    const glide = motionToken(composer, "--desktop-glide") ?? "linear"
    const ease = motionToken(composer, "--desktop-ease") ?? "linear"
    const to = composer.getBoundingClientRect()
    const scale = from.composer.width / to.width
    const dx = from.composer.left + from.composer.width / 2 - (to.left + to.width / 2)
    const dy = from.composer.top + from.composer.height / 2 - (to.top + to.height / 2)
    const travel = durationToken(composer, "--desktop-arrival")
    const moving = composer.animate(
      [
        { transform: `translate(${dx}px, ${dy}px) scale(${scale})` },
        { transform: "none" },
      ],
      { duration: travel, easing: glide },
    )
    // The message leaves the field as the composer travels; its placeholder
    // returns only once the field has nearly settled.
    const clearing = composer
      .querySelector("textarea")
      ?.animate([{ opacity: 0 }, { opacity: 0, offset: 0.6 }, { opacity: 1 }], {
        duration: travel,
        easing: "ease-out",
      })
    const landed = bubble.getBoundingClientRect()
    const rising = bubble.animate(
      [
        {
          transform: `translate(${from.text.left - landed.left - 14}px, ${from.text.top - landed.top - 10}px)`,
          backgroundColor: "transparent",
        },
        { offset: 0.4, backgroundColor: "transparent" },
        { transform: "none" },
      ],
      { duration: travel - 20, easing: glide },
    )
    const settling = title.animate(
      [
        { opacity: 0, transform: "translateY(8px)" },
        { opacity: 1, transform: "none" },
      ],
      {
        duration: durationToken(title, "--desktop-slow"),
        delay: durationToken(title, "--desktop-stagger"),
        easing: ease,
        fill: "backwards",
      },
    )
    // Cancelled on cleanup, so a second mount measures the resting layout
    // rather than a frame of this animation.
    return () =>
      [moving, rising, settling, clearing].forEach((animation) => animation?.cancel())
    // Plays once, for the arrival this conversation mounted with.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
}
