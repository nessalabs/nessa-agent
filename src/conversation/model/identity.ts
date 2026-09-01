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
 * How dilute the icon's paint is.
 *
 * This was `pastel` while the icon was the painting alone on nothing: it had a
 * near-white ground behind it to push against, and thinner paint read as sorbet
 * rather than as a colour. The icon now multiplies the painting into a gradient
 * wash, which drops that ground — and pastel, with nothing behind it, thins to
 * almost nothing by the time the icon is 32px. `soft` is the same paint at the
 * strength the in-app avatar already uses.
 */
export const AGENT_ICON_TONE = "soft" as const

/**
 * The icon's ground: a soft fluid wash, warm through the middle with the
 * lavender pushed out to a rim. Deepest colour first — it floods the box, each
 * later colour is a soft bloom, and the last is centred, which is what puts the
 * warm note in the middle and the cool one at the edges.
 *
 * Chosen from the candidates at /icon.html. It is written here rather than in
 * that preview because the shipped icon is a *hand-authored SVG* — resvg runs
 * no CSS, so the wash cannot be the component — and these are the numbers the
 * two have to agree on. See design.md.
 */
export const AGENT_ICON_WASH = ["#d3a9d8", "#e6b6d2", "#f7bfa6", "#f9a86a"] as const
