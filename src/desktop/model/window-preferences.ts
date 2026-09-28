/**
 * The window's own on-or-off preferences from Settings, as they are stored:
 * each is on unless it was turned off, and anything else stored reads as on —
 * except a preview under Settings › General › Experimental, which is off
 * unless it was turned on (`parseOptIn`). What each one does is said where it
 * is used.
 */
export type Flag = "on" | "off"

/** Reads a stored flag; anything but "off" is on. */
export function parseFlag(value: unknown): Flag {
  return value === "off" ? "off" : "on"
}

/**
 * Reads a preview's stored flag: off unless it was turned on, so a preview
 * reaches only the people who asked for it. Anything else stored — nothing,
 * an old value — reads as off.
 */
export function parseOptIn(value: unknown): Flag {
  return value === "on" ? "on" : "off"
}
