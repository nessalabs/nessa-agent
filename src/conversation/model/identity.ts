/**
 * The one place the agent's face is defined. The generative avatar is
 * deterministic, so the same seed and hue wheel paint the same picture
 * everywhere — the header, the transcript, and the app icon, which is this
 * painting rasterized (see the README's "Regenerating the icon").
 */

/** The identity the painting is derived from. */
export const AGENT_SEED = "nessa"

/**
 * A pastel sorbet wheel — pale rose through lilac into sea — rather than the
 * eight defaults. The default wheel's spread put a near-white wash against the
 * dark ground, which collapsed into a white blob at menu-bar size; these four
 * hues stay in one bright, dilute family instead.
 *
 * The order is load-bearing: the seed picks each wash's hue by index, so
 * sorting this list paints a different picture.
 */
export const AGENT_HUES = [25, 195, 300, 340] as const

/**
 * How dilute the icon's paint is. The in-app avatar keeps the component's
 * default `soft`, which has more pigment to hold its shape at 24px; the icon
 * is thinner, which is what makes it read as sorbet rather than as a colour.
 */
export const AGENT_ICON_TONE = "pastel" as const
