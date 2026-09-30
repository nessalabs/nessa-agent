import { useLayoutEffect, useRef, useState } from "react"

/**
 * A choice of a few words on a glass track, with a soft highlight that
 * slides under the chosen one. Measured from the chosen word, so labels of
 * any width land exactly.
 */
export function Selector<T extends string>({
  value,
  options,
  onChange,
  label,
  size = "md",
}: {
  value: T
  options: readonly { value: T; label: string }[]
  onChange: (value: T) => void
  label: string
  size?: "sm" | "md"
}) {
  const trackRef = useRef<HTMLDivElement>(null)
  const [lens, setLens] = useState<{ x: number; width: number } | null>(null)

  useLayoutEffect(() => {
    const track = trackRef.current
    if (!track) return
    const place = () => {
      const chosen = track.querySelector<HTMLElement>(
        `[data-value="${CSS.escape(value)}"]`,
      )
      if (chosen) setLens({ x: chosen.offsetLeft, width: chosen.offsetWidth })
    }
    place()
    const observer = new ResizeObserver(place)
    observer.observe(track)
    return () => observer.disconnect()
  }, [value])

  return (
    <div
      ref={trackRef}
      className="xp-selector"
      data-size={size}
      role="group"
      aria-label={label}
    >
      {lens ? (
        <span
          aria-hidden
          className="xp-selector-lens"
          style={{ width: lens.width, transform: `translateX(${lens.x}px)` }}
        />
      ) : null}
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          className="xp-selector-option"
          data-value={option.value}
          aria-pressed={option.value === value}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  )
}
