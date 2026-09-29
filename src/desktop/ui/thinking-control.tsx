/**
 * The composer's thinking control: a chip that opens a small popover of the
 * window's glass, holding a slider of thinking effort — a thin track whose
 * fill brightens in the theme's own light towards its top end, and a small
 * knob dragged along it that settles on the nearest level — with the level's
 * name and one line about it cross-fading above, and Fast mode a toggle of
 * its own beside the heading. A model that thinks past Max offers Ultra as a
 * segment of its own at the track's end, where effort is at its most.
 *
 * It is the window's own rather than nessa_ui's `ModelThinkingControl`, whose
 * popover has no place for a level's description, swaps its label rather
 * than cross-fading it, and draws its slider its own way with nothing to
 * restyle it by (ADR 238 › The thinking control). nessa_ui's `ModelFastMode`
 * is the Fast toggle.
 *
 * The levels are `model/composer-options.ts`'s, and every rule of the slider —
 * keys, where the pointer is and which level that is, where the popover sits —
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
import { offeredLevelIndex, type ThinkingLevel } from "../model/composer-options"
import {
  fractionAlong,
  levelAfterKey,
  nearestLevel,
  placePopover,
  positionAt,
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
 * What the words above the slider show, and what they showed before, so the
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
  const index = offeredLevelIndex(levels, value)
  const selected = levels.at(index)
  const unavailable = levels.length === 0
  const [open, setOpen] = useState(false)
  const shown = open && !unavailable
  const triggerRef = useRef<HTMLButtonElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  const knobRef = useRef<HTMLSpanElement>(null)
  const [home, setHome] = useState<HTMLElement | null>(null)
  // Where the knob is held while it is dragged, in levels; null at rest.
  const [dragged, setDragged] = useState<number | null>(null)
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
  const count = levels.length
  const ultraAt = levels.findIndex((level) => level.utmost)

  const close = useCallback((refocus: boolean) => {
    setOpen(false)
    setDragged(null)
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

  // Opened, the keyboard lands on the knob, so the arrows work at once.
  useEffect(() => {
    if (shown) knobRef.current?.focus({ preventScroll: true })
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

  const onKnobKeyDown = (event: KeyboardEvent<HTMLSpanElement>) => {
    const next = levelAfterKey(event.key, index, count)
    if (next === undefined) return
    event.preventDefault()
    choose(next)
  }

  // Pressed anywhere along it, the knob comes to the pointer and follows it,
  // the level following the nearest; let go, it settles on that level.
  const follow = (event: PointerEvent<HTMLDivElement>) => {
    const track = event.currentTarget.getBoundingClientRect()
    const position = positionAt(event.clientX, track.left, track.width, count)
    setDragged(position)
    choose(nearestLevel(position))
  }
  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (!event.isPrimary || event.button !== 0 || count <= 1) return
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    knobRef.current?.focus({ preventScroll: true })
    follow(event)
  }
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (dragged !== null) follow(event)
  }
  const letGo = () => setDragged(null)

  const position = dragged ?? index
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
            'button:not([disabled]), [role="slider"]',
          ),
        ]
        const edge = event.shiftKey ? reachable.at(0) : reachable.at(-1)
        if (event.target !== edge) return
        event.preventDefault()
        close(true)
      }}
      onBlur={(event) => {
        // Focus gone elsewhere closes it; to its own chip it does not, or the
        // chip's click would open it again.
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
                <span className="desktop-thinking-streaks" aria-hidden="true" />
                <DesktopIcon name="fast" />
                <span>Fast</span>
              </>
            }
          />
        ) : null}
      </div>
      {/* The level in words, drawn large; the slider already says it to a screen reader. */}
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
        className="desktop-thinking-slider"
        data-dragging={dragged === null ? undefined : ""}
        data-ultra={ultraAt >= 0 ? "" : undefined}
        style={
          {
            "--position": fractionAlong(position, count),
            // Where Ultra's own segment begins: the level before it, Max.
            "--ultra-from": ultraAt > 0 ? fractionAlong(ultraAt - 1, count) : 1,
          } as CSSProperties
        }
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={letGo}
        onPointerCancel={letGo}
        onLostPointerCapture={letGo}
      >
        <span className="desktop-thinking-rail" aria-hidden="true">
          <span className="desktop-thinking-rest" />
          <span className="desktop-thinking-beyond" />
          <span className="desktop-thinking-glow" />
          <span className="desktop-thinking-clip">
            <span className="desktop-thinking-fill" />
            {selected?.utmost ? (
              // Ultra reached: a light runs once along the fill.
              <span key={`sheen-${reading.turn}`} className="desktop-thinking-sheen" />
            ) : null}
          </span>
          {/* The levels between the ends; where Ultra's segment begins, its gap marks Max. */}
          {levels.map((level, stop) =>
            stop === 0 || stop === count - 1 || stop === ultraAt - 1 ? null : (
              <span
                key={level.value}
                className="desktop-thinking-tick"
                style={{ left: `${fractionAlong(stop, count) * 100}%` }}
              />
            ),
          )}
        </span>
        <span className="desktop-thinking-carriage">
          <span
            ref={knobRef}
            role="slider"
            tabIndex={0}
            aria-labelledby={`${id}-caption`}
            aria-orientation="horizontal"
            aria-valuemin={0}
            aria-valuemax={Math.max(0, count - 1)}
            aria-valuenow={index}
            aria-valuetext={selected?.label}
            aria-describedby={`${id}-says`}
            className="desktop-thinking-knob"
            onKeyDown={onKnobKeyDown}
          />
        </span>
        <span id={`${id}-says`} hidden>
          {selected?.description}
        </span>
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
