/**
 * The composer's vertical budget has one owner, and it is not the chrome.
 *
 * Five independent producers say things above the pill and none of them
 * excludes the others. For a long time the whole arrangement was JSX order in
 * `app.tsx`: nothing bounded how tall the column got, nothing decided which
 * card was read first, and on a short panel the pill was pushed out of the
 * window. `ComposerNotices` now owns both — the room the notices may have and
 * the order they are said in — and `styles.css` owns the ceiling itself.
 *
 * Both are one edit away from being undone by accident. A notice added as a new
 * direct sibling of the box is unbounded again and sits wherever it was pasted;
 * a `max-height` deleted from the stylesheet, or a `min-height` added under it,
 * takes the ceiling with it and nothing fails. These checks are what notices
 * either happening.
 *
 * The JSX is read with the TypeScript parser rather than matched. A first
 * attempt counted angle brackets, and an ordinary fragment broke it in both
 * directions: `<>` is not a tag to that kind of scan while `</>` looks like the
 * end of one, so a fragment anywhere in the composer's subtree ended the scan
 * early and a notice pasted after it was never seen — the check passing while
 * the rule it exists for was being broken. A generic argument and a `<`
 * comparison corrupted it the other way. A check that can be disarmed by
 * unrelated valid code is worse than no check, because it is believed.
 */
import ts from "typescript"

/** The four things that may sit directly in the composer, in order. */
const composerLayout = [
  "ComposerNotices",
  "ConversationQueue",
  "ComposerDeliveryMode",
  "PillComposer",
]

/** How many times the panel's height the notices may be, at most. A third. */
export const noticeShare = 3

const tagName = (node) =>
  ts.isJsxFragment(node)
    ? "<>"
    : (ts.isJsxElement(node) ? node.openingElement : node).tagName.getText()

const isElement = (node) =>
  ts.isJsxElement(node) || ts.isJsxSelfClosingElement(node) || ts.isJsxFragment(node)

function parse(path, source) {
  return ts.createSourceFile(
    path,
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX,
  )
}

/** The opening tag's attributes, whether or not the element has children. */
const attributes = (node) =>
  ts.isJsxElement(node) ? node.openingElement.attributes : node.attributes

function hasClass(node, name) {
  if (ts.isJsxFragment(node)) return false
  return attributes(node).properties.some(
    (property) =>
      ts.isJsxAttribute(property) &&
      property.name.getText() === "className" &&
      property.initializer !== undefined &&
      ts.isStringLiteral(property.initializer) &&
      property.initializer.text === name,
  )
}

function find(node, matches) {
  if (isElement(node) && matches(node)) return node
  let found
  node.forEachChild((child) => {
    found ??= find(child, matches)
  })
  return found
}

/**
 * The elements a JSX child position actually renders.
 *
 * `{generating && <ComposerDeliveryMode />}` renders one element through an
 * expression, and an expression holding nothing but a comment renders nothing
 * at all, so the expression is walked — but only as far as the first element in
 * it. Anything below that is that element's own business, which is exactly how
 * a notice handed to `ComposerNotices` as a prop is inside the box rather than
 * beside it.
 */
function rendered(node) {
  if (isElement(node)) return [node]
  if (ts.isJsxText(node)) return node.text.trim() === "" ? [] : [node]
  const found = []
  node.forEachChild((child) => {
    found.push(...rendered(child))
  })
  return found
}

/**
 * The element names directly under `<div className="nessa-composer">`, or null
 * when that element is not there — so a rename fails loudly rather than passing
 * on an empty list.
 */
export function composerChildren(path, source) {
  const composer = find(parse(path, source), (node) => hasClass(node, "nessa-composer"))
  if (!composer || !ts.isJsxElement(composer)) return null
  return composer.children
    .flatMap(rendered)
    .map((child) => (ts.isJsxText(child) ? "text" : tagName(child)))
}

/**
 * Every notice in the chrome is inside the box.
 *
 * The list of direct children above catches a notice pasted in beside the box.
 * This catches the same notice handed to something else that renders it — a
 * banner prop on the queue, say — which the child list cannot see. It knows
 * `AgentNotification` by that name, so a notice under a new name of its own is
 * caught by the child list instead, as a child that does not belong. What
 * neither sees is a notice rendered inside another module; this rule is about
 * the chrome.
 */
function noticesOutsideTheBox(path, source) {
  const strays = []
  const walk = (node, inside) => {
    const own = isElement(node) && tagName(node) === "ComposerNotices"
    if (isElement(node) && !inside && tagName(node) === "AgentNotification") {
      const file = node.getSourceFile()
      strays.push(ts.getLineAndCharacterOfPosition(file, node.getStart(file)).line + 1)
    }
    node.forEachChild((child) => walk(child, inside || own))
  }
  walk(parse(path, source), false)
  return strays
}

export function composerNoticeViolations(path, source) {
  if (path !== "src/panel/ui/app.tsx") return []
  const failures = []
  const children = composerChildren(path, source)
  if (children === null) {
    failures.push(
      'the composer element no longer carries className="nessa-composer"; move this check with it',
    )
  } else if (children.join(" ") !== composerLayout.join(" ")) {
    failures.push(
      `the composer holds ${children.join(", ") || "nothing"}; it holds exactly ${composerLayout.join(", ")}, because everything said above the pill goes through ComposerNotices, which owns the room the notices may have and the order they are said in. The queue badge and the delivery row are outside it on purpose: they are controls rather than statements, one short row each, and a control the composer is about to obey must not be somewhere you have to scroll to find`,
    )
  }
  const strays = noticesOutsideTheBox(path, source)
  if (strays.length > 0) {
    failures.push(
      `a notice is rendered outside ComposerNotices (line ${strays.join(", ")}); every notice the chrome says goes through the box, which is what bounds the column and what decides the order`,
    )
  }
  return failures
}

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
 * Every rule that styles the notice box, not merely the first one written.
 *
 * The cascade is what takes a ceiling away in practice: a later rule with a
 * heavier selector, a media query for short windows, a `min-height` that beats
 * the `max-height` outright. Reading one block and stopping would pass all
 * three. `:empty` is excluded because that rule is about the box not being
 * there at all.
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
