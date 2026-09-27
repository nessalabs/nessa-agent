import * as React from "react"

/**
 * What the Messages icon is painted with. Every wash is one of these at its own
 * strength, so a palette swap recolours the whole painting consistently.
 */
export interface MessagesIconColors {
  /** The deepest blue: the ground's darkest corner and the pooled wash at left. */
  deep: string
  /** The ground's middle, and the diagonal band across it. */
  mid: string
  /** The lavender pool behind the bubbles and the ground's far corner. */
  lavender: string
  /** The pale sheet falling from the top edge. */
  light: string
  /** The nearer, stronger wave along the bottom. */
  wave: string
  /** The farther, paler wave. */
  foam: string
  /** The bubbles' lit edge. */
  bubble: string
  /** The bubbles' shaded edge. */
  bubbleShade: string
}

export const messagesIconColors: MessagesIconColors = {
  deep: "#1257ff",
  mid: "#4a82ff",
  lavender: "#8395f7",
  light: "#c8d6fd",
  wave: "#8ccaf8",
  foam: "#c4e9f7",
  bubble: "#f5f8ff",
  bubbleShade: "#c7d9fb",
}

/**
 * The Messages icon: two speech bubbles over a watercolour pool of blues, as a
 * vector so one drawing serves every size, and recolourable through `colors`.
 *
 * Drawn on a 100-unit circle. The washes are translucent shapes clipped to it,
 * layered back to front the way the painting was: ground, pools, falling
 * sheet, waves, then the bubbles. Each bubble is its body and its tail in one
 * group with the opacity on the group, so where they overlap it is one shape
 * rather than a darker seam. Ids are per instance, so several icons on one
 * page do not borrow each other's gradients.
 */
export function MessagesIcon({
  colors,
  title,
  ...props
}: Omit<React.ComponentProps<"svg">, "children"> & {
  colors?: Partial<MessagesIconColors>
  /** Names the icon for assistive technology; without it the icon is decorative. */
  title?: string
}) {
  const paint = { ...messagesIconColors, ...colors }
  const id = React.useId().replace(/:/g, "")
  const ground = `${id}-ground`
  const front = `${id}-front`
  const back = `${id}-back`
  const clip = `${id}-clip`
  return (
    <svg
      viewBox="0 0 100 100"
      xmlns="http://www.w3.org/2000/svg"
      role={title ? "img" : undefined}
      aria-hidden={title ? undefined : true}
      {...props}
    >
      {title ? <title>{title}</title> : null}
      <defs>
        <linearGradient id={ground} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor={paint.mid} />
          <stop offset="0.4" stopColor={paint.deep} stopOpacity="0.85" />
          <stop offset="1" stopColor={paint.lavender} />
        </linearGradient>
        <linearGradient id={front} x1="0.15" y1="0.1" x2="0.85" y2="0.95">
          <stop offset="0" stopColor={paint.bubble} />
          <stop offset="1" stopColor={paint.bubbleShade} />
        </linearGradient>
        <linearGradient id={back} x1="0" y1="0" x2="0.8" y2="1">
          <stop offset="0" stopColor={paint.bubble} />
          <stop offset="1" stopColor={paint.bubbleShade} />
        </linearGradient>
        <clipPath id={clip}>
          <circle cx="50" cy="50" r="50" />
        </clipPath>
      </defs>
      <g clipPath={`url(#${clip})`}>
        <rect width="100" height="100" fill={`url(#${ground})`} />
        {/* The pool of deep blue pooled at the left. */}
        <path
          d="M7 52C4 36 16 19 31 14c9-3 12 8 8 24-4 16-15 25-24 23-5-1-7-5-8-9Z"
          fill={paint.deep}
          opacity="0.75"
        />
        {/* The lavender pool the bubbles sit in. */}
        <path
          d="M24 25C39 13 71 12 86 23c11 8 9 24-4 34-11 8-27 5-33-6-5-10-15-20-25-26Z"
          fill={paint.lavender}
          opacity="0.55"
        />
        {/* The pale sheet falling from the top edge to the right. */}
        <path
          d="M46 0c5 13 13 22 16 37 3 17 17 28 38 33V0Z"
          fill={paint.light}
          opacity="0.45"
        />
        {/* The diagonal band that lightens the lower half. */}
        <path
          d="M0 80c19-9 37-17 55-20 16-3 31-4 45-12v52H0Z"
          fill={paint.mid}
          opacity="0.3"
        />
        {/* The waves along the bottom, nearer then farther. */}
        <path
          d="M14 96c10-13 25-19 38-13 10 5 20 10 48-3v20H0Z"
          fill={paint.wave}
          opacity="0.8"
        />
        <path
          d="M36 100c10-10 24-15 39-17 10-1 18-5 25-9v26Z"
          fill={paint.foam}
          opacity="0.7"
        />
      </g>
      {/* The farther bubble, lower right, its tail to the right. */}
      <g opacity="0.72">
        <ellipse cx="63.5" cy="60.5" rx="18.3" ry="17" fill={`url(#${back})`} />
        <path
          d="M72 72c3 2 4.6 4 5.4 5.6.6 1 1.8.5 1.7-.9-.4-2.6-.9-5 .6-7.7Z"
          fill={`url(#${back})`}
        />
      </g>
      {/* The nearer bubble, its tail to the lower left. */}
      <g opacity="0.92">
        <ellipse cx="45.8" cy="46.5" rx="23.4" ry="18.3" fill={`url(#${front})`} />
        <path
          d="M30 56.5c1.2 4-1.7 8-2.5 11.4-.3 1.3.9 1.7 2.4.9 3-1.7 5.5-4.3 9.3-5.4Z"
          fill={`url(#${front})`}
        />
      </g>
    </svg>
  )
}
