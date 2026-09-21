/**
 * The ceiling over the composer's notices, and the ways it gets lost.
 *
 * Five independent producers say things above the pill and none of them
 * excludes the others, so without a bound that column pushed the composer out
 * of a short panel. `.nessa-composer-notices` in `src/styles.css` is the bound:
 * a third of the panel's own height, and it scrolls. Nothing else in the
 * architecture check reads CSS, so nothing else would notice that rule being
 * deleted, outweighed, or quietly turned off in a media query.
 *
 * This file is pure text on purpose. `scripts/check-architecture.mjs` runs on
 * the Rust jobs with bare Node and no `node_modules`, so nothing here may
 * import a parser. The other half of the budget — that every notice goes
 * through one box, which owns the order too — is about JSX, needs a real
 * parser, and is a lint rule instead: `scripts/eslint/composer-notices.mjs`.
 */

/** How many times the panel's height the notices may be, at most. A third. */
export const noticeShare = 3

/** Comments may name a selector or a property; only declarations count. */
const withoutComments = (styles) => styles.replace(/\/\*[\s\S]*?\*\//g, "")

const capValue =
  /^calc\(\s*var\(\s*--nessa-window-height\s*(?:,\s*([^)]*?)\s*)?\)\s*\/\s*([\d.]+)\s*\)$/

function declarations(body) {
  return body.split(";").flatMap((piece) => {
    const at = piece.indexOf(":")
    if (at === -1) return []
    return [
      {
        property: piece.slice(0, at).trim().toLowerCase(),
        value: piece.slice(at + 1).trim(),
      },
    ]
  })
}

/**
 * Every rule that names the notice box, not merely the first one written.
 *
 * The cascade is what takes a ceiling away in practice: a later rule with a
 * heavier selector, a media query for short windows, a `min-height` that beats
 * the `max-height` outright. Reading one block and stopping would pass all
 * three. `:empty` is excluded because that rule is about the box not being
 * there at all. This reads selectors rather than resolving the cascade, so a
 * rule that reaches the box without naming it is beyond it.
 */
export function noticeRules(styles) {
  return [...withoutComments(styles).matchAll(/([^{}]*)\{([^{}]*)\}/g)]
    .map((rule) => ({ selector: rule[1].trim(), body: rule[2] }))
    .filter(
      (rule) =>
        /\.nessa-composer-notices(?![\w-])/.test(rule.selector) &&
        !/:empty\b/.test(rule.selector),
    )
}

/** What the box may be at a given panel height, taking the loosest ceiling declared. */
export function declaredNoticeCap(styles) {
  const divisors = noticeRules(styles).flatMap((rule) =>
    declarations(rule.body).flatMap((declaration) => {
      if (declaration.property !== "max-height") return []
      const cap = capValue.exec(declaration.value)
      return cap ? [Number.parseFloat(cap[2])] : []
    }),
  )
  if (divisors.length === 0 || divisors.some((divisor) => !(divisor > 0))) return null
  return (windowHeight) => windowHeight / Math.min(...divisors)
}

/** The fallback each ceiling uses before the host has reported a window size. */
export function declaredCapFallbacks(styles) {
  return noticeRules(styles).flatMap((rule) =>
    declarations(rule.body).flatMap((declaration) => {
      if (declaration.property !== "max-height") return []
      const cap = capValue.exec(declaration.value)
      return cap ? [{ selector: rule.selector, fallback: cap[1] ?? "" }] : []
    }),
  )
}

/** The last word on whether the box scrolls, shorthand included. */
function overflowY(body) {
  let value = null
  for (const declaration of declarations(body)) {
    if (declaration.property === "overflow-y") value = declaration.value
    else if (declaration.property === "overflow") {
      const axes = declaration.value.split(/\s+/)
      value = axes[1] ?? axes[0]
    }
  }
  return value
}

export function composerBudgetViolations(styles) {
  const rules = noticeRules(styles)
  if (rules.length === 0) {
    return [
      ".nessa-composer-notices has no rule; the notices above the pill are unbounded again and a short panel loses the composer",
    ]
  }
  const failures = []
  let capped = false
  let scrolls = false
  for (const rule of rules) {
    for (const declaration of declarations(rule.body)) {
      if (declaration.property === "min-height" || declaration.property === "height") {
        failures.push(
          `${rule.selector} sets ${declaration.property}: ${declaration.value}; a floor beats the ceiling above it, which is the cap gone with nothing to show for it`,
        )
      }
      if (declaration.property !== "max-height") continue
      const cap = capValue.exec(declaration.value)
      if (!cap) {
        failures.push(
          `${rule.selector} sets max-height: ${declaration.value}; the ceiling is calc(var(--nessa-window-height, …) / n), because a fixed number is wrong at one of 320px and 900px and anything else is not a ceiling`,
        )
      } else if (Number.parseFloat(cap[2]) < noticeShare) {
        failures.push(
          `${rule.selector} gives the notices a ${(100 / Number.parseFloat(cap[2])).toFixed(0)}% share of the panel; a third is what leaves the transcript and the pill the rest at the window's minimum height`,
        )
      } else {
        capped = true
      }
    }
    const overflow = overflowY(rule.body)
    if (overflow === null) continue
    if (/^(auto|scroll)$/.test(overflow)) scrolls = true
    else {
      failures.push(
        `${rule.selector} sets the vertical overflow to ${overflow}; the cap must scroll, or it clips a card and takes its Retry off the screen for good`,
      )
    }
  }
  if (!capped) {
    failures.push(
      "the notices need a max-height derived from --nessa-window-height; without one the column pushes the pill out of a short window",
    )
  }
  if (!scrolls) {
    failures.push(
      "the notices must scroll under their ceiling; without overflow-y the cap clips a card and takes its Retry off the screen for good",
    )
  }
  if (
    !/\.nessa-composer-notices:empty\s*\{[^}]*display:\s*none/.test(
      withoutComments(styles),
    )
  ) {
    failures.push(
      "an empty notice strip must be display:none, or a silent composer keeps a tab stop and an announced group nobody asked for",
    )
  }
  return failures
}
