/**
 * What on the page holds the keyboard as its own: a dialog or a menu. Focus
 * inside one is left where it is, and Escape inside one is its own — ADR
 * 238's first owner — whatever else would answer it (`workspace/adapters/dom/focus.ts`,
 * `widget-escape.ts`, `use-edge-peek.ts`).
 */
export const modalSelector = '[role="dialog"], [role="menu"], [aria-modal="true"]'

/**
 * Whether `target` lies inside a dialog or a menu — one over `surface`, when
 * a surface is named: a surface that is itself a dialog (Settings) holds its
 * own parts inside it, and they are not under anything.
 */
export function inModal(target: EventTarget | null, surface?: Element | null): boolean {
  const modal = target instanceof Element ? target.closest(modalSelector) : null
  return modal !== null && !(surface && modal.contains(surface))
}
