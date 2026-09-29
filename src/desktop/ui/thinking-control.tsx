/**
 * The composer's thinking control: a chip that opens a small popover of the
 * window's glass, in which a track of stops — one per level the model
 * offers — fills and brightens in the theme's own light as the level rises,
 * the level's name and one line about it cross-fading above, and Fast mode
 * a toggle of its own beside the heading.
 *
 * It is the window's own rather than nessa_ui's `ModelThinkingControl`, whose
 * popover has no place for a level's description and swaps its label rather
 * than cross-fading it, and draws a continuous slider rather than stops (ADR
 * 238 › The thinking control). nessa_ui's `ModelFastMode` is the Fast toggle.
 *
 * The levels are `model/composer-options.ts`'s, and every rule of the track —
 * keys, the pointer, the order its stops light in, where the popover sits —
 * is `model/thinking-effort.ts`'s. Motion is transform and opacity, on the
 * window's tokens, and none at all with less motion (`thinking-control.css`).
 * The chip is the same size whatever the level, so the composer never moves
 * when it changes.
 */
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
} from "react"
import { createPortal } from "react-dom"
import { ModelFastMode } from "@nessa-ui/react/model-capability-controls"
import type { ThinkingLevel } from "../model/composer-options"
import {
  levelAfterKey,
  placePopover,
  stopAt,
  stopDelays,
  stopRank,
} from "../model/thinking-effort"
import { DesktopIcon } from "./icons"
import "./thinking-control.css"

export interface ThinkingControlProps {
  /** The levels the model offers, least first; none, and the chip is disabled. */
  levels: readonly ThinkingLevel[]
  value: string
  onValueChange: (value: string) => void
  /** Offered only on a model that has Fast mode. */
  fastMode?: { pressed: boolean; onPressedChange: (pressed: boolean) => void }
}

/**
 * What the words above the track show, and what they showed before, so the
 * two cross-fade; `turn` counts the changes, so each one's fade is new.
 */
interface Reading {
  readonly index: number
  readonly previous: number | undefined
  readonly turn: number
}

export function ThinkingControl({
  levels,
  value,
  onValueChange,
  fastMode,
}: ThinkingControlProps) {
  const found = levels.findIndex((level) => level.value === value)
  const index = found >= 0 ? found : 0
  const selected = levels.at(index)
  const unavailable = levels.length === 0
  const [open, setOpen] = useState(false)
  const shown = open && !unavailable
  const triggerRef = useRef<HTMLButtonElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  const stopRefs = useRef<(HTMLButtonElement | null)[]>([])
  const [home, setHome] = useState<HTMLElement | null>(null)
  const id = useId()

  // The words follow the level; the level before stays, fading, beneath them.
  const [reading, setReading] = useState<Reading>({
    index,
    previous: undefined,
    turn: 0,
  })
  if (reading.index !== index)
    setReading({ index, previous: reading.index, turn: reading.turn + 1 })
  const previous = reading.previous === undefined ? undefined : levels[reading.previous]
  const rising = reading.previous === undefined || index > reading.previous
  const delays = stopDelays(reading.previous ?? index, index, levels.length)

  const close = useCallback((refocus: boolean) => {
    setOpen(false)
    if (refocus) triggerRef.current?.focus({ preventScroll: true })
  }, [])

  const choose = (next: number) => {
    const level = levels.at(next)
    if (!level || next === index) return
    onValueChange(level.value)
  }

  // Placed before it paints, and again when the window changes size; the
  // first frame of its rise is already in its place.
  useLayoutEffect(() => {
    if (!shown) return
    const place = () => {
      const trigger = triggerRef.current
      const content = contentRef.current
      if (!trigger || !content) return
      const anchor = trigger.getBoundingClientRect()
      const placed = placePopover(
        {
          left: anchor.left,
          top: anchor.top,
          width: anchor.width,
          height: anchor.height,
        },
        { width: content.offsetWidth, height: content.offsetHeight },
        { width: window.innerWidth, height: window.innerHeight },
      )
      content.style.left = `${placed.x}px`
      content.style.top = `${placed.y}px`
      content.dataset.side = placed.side
    }
    place()
    window.addEventListener("resize", place)
    return () => window.removeEventListener("resize", place)
  }, [shown, home])

  // Opened, the keyboard lands on the level chosen, so the arrows work at once.
  useEffect(() => {
    if (!shown) return
    contentRef.current
      ?.querySelector<HTMLElement>('[role="radio"][aria-checked="true"]')
      ?.focus({ preventScroll: true })
  }, [shown])

  // A press anywhere else closes it, leaving focus to whatever was pressed.
  useEffect(() => {
    if (!shown) return
    const onPointerDown = (event: globalThis.PointerEvent) => {
      const target = event.target
      if (!(target instanceof Node)) return
      if (contentRef.current?.contains(target) || triggerRef.current?.contains(target))
        return
      close(false)
    }
    document.addEventListener("pointerdown", onPointerDown, true)
    return () => document.removeEventListener("pointerdown", onPointerDown, true)
  }, [shown, close])

  const onTrackKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = levelAfterKey(event.key, index, levels.length)
    if (next === undefined) return
    event.preventDefault()
    choose(next)
    stopRefs.current[next]?.focus({ preventScroll: true })
  }

  // The track is also dragged along, like a slider: the stop under the pointer is the level.
  const dragging = useRef(false)
  const follow = (event: PointerEvent<HTMLDivElement>) => {
    const track = event.currentTarget.getBoundingClientRect()
    const next = stopAt(event.clientX, track.left, track.width, levels.length)
    choose(next)
    return next
  }
  const onTrackPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (!event.isPrimary || event.button !== 0) return
    event.preventDefault()
    dragging.current = true
    event.currentTarget.setPointerCapture(event.pointerId)
    const next = follow(event)
    stopRefs.current[next]?.focus({ preventScroll: true })
  }
  const onTrackPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return
    const next = follow(event)
    stopRefs.current[next]?.focus({ preventScroll: true })
  }
  const endDrag = () => {
    dragging.current = false
  }

  const popover = shown ? (
    <div
      ref={contentRef}
      id={`${id}-popover`}
      role="dialog"
      aria-label="Thinking"
      className="desktop-popover desktop-thinking"
      data-state="open"
      data-utmost={selected?.utmost ? "" : undefined}
      data-rising={rising ? "" : undefined}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          // Taken here, so nothing behind the popover also answers it.
          event.preventDefault()
          event.stopPropagation()
          close(true)
          return
        }
        if (event.key !== "Tab") return
        // Tab past either end leaves the popover for its chip, closing it,
        // rather than for wherever the page's order would drop it.
        const reachable = [
          ...event.currentTarget.querySelectorAll<HTMLElement>(
            'button:not([disabled]):not([tabindex="-1"])',
          ),
        ]
        const edge = event.shiftKey ? reachable.at(0) : reachable.at(-1)
        if (event.target !== edge) return
        event.preventDefault()
        close(true)
      }}
      onBlur={(event) => {
        // Focus gone elsewhere — Tab past its end, say — closes it; to its own
        // chip it does not, or the chip's click would open it again.
        const next = event.relatedTarget
        if (
          next instanceof Node &&
          !event.currentTarget.contains(next) &&
          !triggerRef.current?.contains(next)
        )
          close(false)
      }}
    >
      <span className="desktop-thinking-bloom" aria-hidden="true" />
      <div className="desktop-thinking-head">
        <span className="desktop-thinking-caption" id={`${id}-caption`}>
          Thinking
        </span>
        {fastMode ? (
          <ModelFastMode
            className="desktop-thinking-fast"
            pressed={fastMode.pressed}
            onPressedChange={fastMode.onPressedChange}
            icon={
              <>
                <DesktopIcon name="fast" />
                <span>Fast</span>
              </>
            }
          />
        ) : null}
      </div>
      {/* What the chosen stop says, drawn large; its radio already says it to a screen reader. */}
      <div className="desktop-thinking-reading" aria-hidden="true">
        {previous ? (
          <span
            key={`leaving-${reading.turn}`}
            className="desktop-thinking-words"
            data-leaving=""
          >
            <span className="desktop-thinking-name">{previous.label}</span>
            <span className="desktop-thinking-description">{previous.description}</span>
          </span>
        ) : null}
        <span
          key={`shown-${reading.turn}`}
          className="desktop-thinking-words"
          data-entering={reading.previous === undefined ? undefined : ""}
        >
          <span className="desktop-thinking-name">{selected?.label}</span>
          <span className="desktop-thinking-description">{selected?.description}</span>
        </span>
      </div>
      <div
        role="radiogroup"
        aria-labelledby={`${id}-caption`}
        className="desktop-thinking-track"
        onKeyDown={onTrackKeyDown}
        onPointerDown={onTrackPointerDown}
        onPointerMove={onTrackPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onLostPointerCapture={endDrag}
      >
        {levels.map((level, stop) => (
          <button
            key={level.value}
            ref={(element) => {
              stopRefs.current[stop] = element
            }}
            type="button"
            role="radio"
            aria-checked={stop === index}
            aria-label={level.label}
            aria-describedby={`${id}-says-${stop}`}
            tabIndex={stop === index ? 0 : -1}
            // A screen reader's activation is a click with no pointer before it.
            onClick={() => choose(stop)}
            className="desktop-thinking-stop"
            data-lit={stop <= index ? "" : undefined}
            data-current={stop === index ? "" : undefined}
            data-utmost={level.utmost ? "" : undefined}
            style={
              {
                "--stop-rank": stopRank(stop, levels.length),
                "--stop-delay": delays[stop],
              } as CSSProperties
            }
          >
            <span className="desktop-thinking-bar">
              <span className="desktop-thinking-fill" />
              {selected?.utmost && stop <= index ? (
                // Max reached: a light runs once along the lit track.
                <span key={`sheen-${reading.turn}`} className="desktop-thinking-sheen" />
              ) : null}
            </span>
            <span id={`${id}-says-${stop}`} hidden>
              {level.description}
            </span>
          </button>
        ))}
      </div>
    </div>
  ) : null

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className="desktop-chip desktop-chip-thinking"
        aria-label={
          selected ? `Thinking level: ${selected.label}` : "Thinking levels unavailable"
        }
        aria-haspopup="dialog"
        aria-expanded={shown}
        aria-controls={shown ? `${id}-popover` : undefined}
        data-state={shown ? "open" : "closed"}
        data-fast={fastMode ? (fastMode.pressed ? "on" : "off") : undefined}
        disabled={unavailable}
        onClick={(event) => {
          if (shown) return close(false)
          setHome(event.currentTarget.closest<HTMLElement>("[data-surface]"))
          setOpen(true)
        }}
      >
        <DesktopIcon name="thinking" />
        {fastMode ? (
          // Laid out wherever Fast is offered, so turning it on moves nothing.
          <DesktopIcon name="fast" className="desktop-fast-mark" />
        ) : null}
      </button>
      {popover ? createPortal(popover, home ?? document.body) : null}
    </>
  )
}
