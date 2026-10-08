/**
 * The window's own on-or-off preferences from Settings, as they are stored:
 * each is on unless it was turned off, and anything else stored reads as on.
 * What each one does is said where it is used.
 */
export type Flag = "on" | "off"

/** Reads a stored flag; anything but "off" is on. */
export function parseFlag(value: unknown): Flag {
  return value === "off" ? "off" : "on"
}

/**
 * Reads a stored opt-in: a preview under Settings › Advanced › Experimental,
 * off until it is turned on, and anything else stored reads as off.
 */
export function parseOptIn(value: unknown): Flag {
  return value === "on" ? "on" : "off"
}
