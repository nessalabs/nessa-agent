/**
 * The window's tooltip, drawn in its own glass rather than the system's plain
 * box. A control asks for it with `tooltip(...)` (`ui/tooltip.ts`); nothing
 * else about the control changes.
 *
 * One tooltip for the whole window, placed and filled here rather than
 * rendered per control, so hovering never renders a component. It behaves
 * as macOS's do:
 *
 * - it shows after a rest on a control, or when the keyboard (not a press)
 *   focuses one; once one has shown, the next shows at once for a moment;
 * - a press, a key, Escape, a scroll, the pointer leaving or the window
 *   losing focus hides it;
 * - it never shows while the control's own menu is open, or while a button
 *   is held — a drag, a resize.
 *
 * Placement is `model/tooltip-placement.ts`, clear of the traffic lights;
 * the look is `.desktop-tooltip` in `styles.css`. It is drawn inside the
 * surface of the control it names, so it wears that surface's theme.
 */
import { useEffect } from "react"
import { placeTooltip, type Box, type TooltipSide } from "../model/tooltip-placement"

/** How long the pointer rests on a control before its tooltip shows. */
const restMs = 500
/** How long after one tooltip hides the next shows at once. */
const warmMs = 300
/** Where the traffic lights sit in a macOS window, which no tooltip covers. */
const trafficLights: Box = { left: 0, top: 0, width: 84, height: 36 }

/** The box a control is drawn in; a wrapper that only groups its child is measured by the child. */
function boxOf(target: HTMLElement): DOMRect {
  const drawn =
    getComputedStyle(target).display === "contents" ? target.firstElementChild : null
  return (drawn ?? target).getBoundingClientRect()
}

/** Whether a menu or popover the control opens is open now. */
function menuOpen(target: HTMLElement): boolean {
  const open = '[data-state="open"], [aria-haspopup][aria-expanded="true"]'
  return target.matches(open) || target.querySelector(open) !== null
}

const sides: readonly TooltipSide[] = ["below", "above", "right", "left"]
const sideOf = (value: string | undefined): TooltipSide =>
  sides.find((side) => side === value) ?? "below"

export function useWindowTooltips(): void {
  useEffect(() => {
    const tip = document.createElement("div")
    tip.className = "desktop-tooltip"
    tip.setAttribute("role", "tooltip")
    tip.id = "desktop-tooltip"
    tip.hidden = true
    const label = document.createElement("span")
    const key = document.createElement("span")
    key.className = "desktop-tooltip-shortcut"
    tip.append(label, key)

    let anchor: HTMLElement | null = null
    // The control just pressed: it shows nothing more until the pointer leaves it.
    let pressed: HTMLElement | null = null
    let timer = 0
    let warmUntil = 0

    const show = (target: HTMLElement) => {
      const text = target.dataset.tooltip ?? ""
      if (!target.isConnected || !text || menuOpen(target)) return
      label.textContent = text
      const shortcut = target.dataset.tooltipShortcut ?? ""
      key.textContent = shortcut
      key.hidden = shortcut === ""
      // Drawn inside the surface the control sits in — the workspace, Settings,
      // the classic shell — so it takes that surface's tokens and theme.
      const surface = target.closest<HTMLElement>("[data-surface]")
      const home = surface ?? document.body
      if (tip.parentElement !== home) home.append(tip)
      tip.hidden = false
      const lights =
        surface?.dataset.host === "macos" && surface.dataset.surface === "window"
          ? trafficLights
          : undefined
      const placed = placeTooltip(
        boxOf(target),
        { width: tip.offsetWidth, height: tip.offsetHeight },
        { width: window.innerWidth, height: window.innerHeight },
        { prefer: sideOf(target.dataset.tooltipSide), avoid: lights },
      )
      tip.dataset.side = placed.side
      tip.style.transform = `translate(${placed.x}px, ${placed.y}px)`
      // Replayed each time it shows: the fade belongs to this showing.
      delete tip.dataset.shown
      void tip.offsetWidth
      tip.dataset.shown = ""
      // Describes the control only where its words say more than its name.
      if (!(target.getAttribute("aria-label") ?? "").includes(text))
        target.setAttribute("aria-describedby", tip.id)
    }

    const hide = () => {
      window.clearTimeout(timer)
      // Nothing shown or waiting: touch nothing, so a key press lays nothing out again.
      if (tip.hidden && anchor === null) return
      if (anchor?.getAttribute("aria-describedby") === tip.id)
        anchor.removeAttribute("aria-describedby")
      if (!tip.hidden) warmUntil = performance.now() + warmMs
      anchor = null
      tip.hidden = true
      delete tip.dataset.shown
    }

    const aim = (target: HTMLElement | null) => {
      if (target === anchor) return
      hide()
      if (!target) return
      anchor = target
      const wait = performance.now() < warmUntil ? 0 : restMs
      timer = window.setTimeout(() => {
        if (anchor === target) show(target)
      }, wait)
    }

    const tooltipped = (node: EventTarget | null) =>
      node instanceof Element ? node.closest<HTMLElement>("[data-tooltip]") : null

    const onPointerOver = (event: PointerEvent) => {
      // A held button is a drag or a resize under way; a touch has no hover.
      if (event.pointerType === "touch" || event.buttons !== 0) return
      const target = tooltipped(event.target)
      if (target !== pressed) aim(target)
    }
    const onPointerOut = (event: PointerEvent) => {
      const to = event.relatedTarget as Node | null
      if (pressed && !pressed.contains(to)) pressed = null
      if (anchor && !anchor.contains(to)) hide()
    }
    const onPointerDown = (event: PointerEvent) => {
      pressed = tooltipped(event.target)
      hide()
    }
    const onFocusIn = (event: FocusEvent) => {
      const focused = event.target
      // Only focus the keyboard gave: a press already shows what it pressed.
      if (focused instanceof Element && focused.matches(":focus-visible"))
        aim(tooltipped(focused))
    }
    const onKeyDown = (event: KeyboardEvent) => {
      // Moving on with Tab is how the next control's tooltip comes; any other key acts.
      if (event.key === "Tab" || event.key === "Shift") return
      hide()
    }

    window.addEventListener("pointerover", onPointerOver)
    window.addEventListener("pointerout", onPointerOut)
    window.addEventListener("pointerdown", onPointerDown, true)
    window.addEventListener("focusin", onFocusIn)
    window.addEventListener("focusout", hide)
    window.addEventListener("keydown", onKeyDown, true)
    window.addEventListener("scroll", hide, true)
    window.addEventListener("blur", hide)
    return () => {
      hide()
      window.removeEventListener("pointerover", onPointerOver)
      window.removeEventListener("pointerout", onPointerOut)
      window.removeEventListener("pointerdown", onPointerDown, true)
      window.removeEventListener("focusin", onFocusIn)
      window.removeEventListener("focusout", hide)
      window.removeEventListener("keydown", onKeyDown, true)
      window.removeEventListener("scroll", hide, true)
      window.removeEventListener("blur", hide)
      tip.remove()
    }
  }, [])
}
