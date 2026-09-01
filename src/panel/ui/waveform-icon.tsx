import * as React from "react"

/**
 * The iMessage-style voice-input glyph, ported from the design system's
 * pill-composer story, where it is defined locally rather than exported.
 *
 * Drawn in `currentColor`, so the composer action owns its colour. While
 * `active` the bars pulse like a live level meter, which lets a running
 * recording read as recording without swapping the icon out from under it.
 */
export function WaveformIcon({
  className,
  active = false,
}: {
  className?: string
  active?: boolean
}) {
  const ref = React.useRef<SVGSVGElement>(null)

  React.useEffect(() => {
    const node = ref.current
    if (!node || !active) return
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return

    const animations = Array.from(node.querySelectorAll("rect")).map((bar, index) =>
      bar.animate(
        [
          { transform: "scaleY(0.7)" },
          { transform: "scaleY(1.15)" },
          { transform: "scaleY(0.7)" },
        ],
        {
          duration: 1400,
          delay: index * 150,
          iterations: Infinity,
          easing: "ease-in-out",
        },
      ),
    )
    return () => animations.forEach((animation) => animation.cancel())
  }, [active])

  // x, y, height, and whether the bar is one of the dimmed outer ones.
  const bars: [number, number, number, boolean][] = [
    [1.25, 7.5, 3, true],
    [4.25, 3, 12, false],
    [7.25, 5, 8, true],
    [10.25, 2, 14, false],
    [13.25, 5, 8, true],
    [16.25, 7.5, 3, false],
  ]

  return (
    <svg
      ref={ref}
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 18 18"
      aria-hidden="true"
      className={className}
    >
      {bars.map(([x, y, height, soft]) => (
        <rect
          key={x}
          x={x - 0.75}
          y={y}
          width="1.5"
          height={height}
          rx="0.75"
          fill="currentColor"
          fillOpacity={soft ? 0.4 : 1}
          style={{ transformBox: "fill-box", transformOrigin: "center" }}
        />
      ))}
    </svg>
  )
}
