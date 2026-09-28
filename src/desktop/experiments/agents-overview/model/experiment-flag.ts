/**
 * An experiment in Settings › General › Experimental, as it is stored: off
 * unless it was turned on, so a preview reaches only the people who asked
 * for it. Anything else stored — nothing, an old value — reads as off.
 */
import type { Flag } from "../../../model/window-preferences"

export function parseExperimentFlag(value: unknown): Flag {
  return value === "on" ? "on" : "off"
}
