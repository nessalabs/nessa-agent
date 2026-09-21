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
 * Both are one edit away from being undone by accident. A sixth notice pasted
 * into `.nessa-composer` beside the box is unbounded again and sits wherever it
 * was pasted; a `max-height` deleted from the stylesheet takes the ceiling with
 * it and nothing fails. These two checks are what notices either happening.
 */

/** Blank out what is not JSX structure, so the tag scan cannot trip over a `<` in prose or a string. */
export function maskJsxNonCode(source) {
  let masked = ""
  let index = 0
  while (index < source.length) {
    const rest = source.slice(index)
    const blockComment = /^\/\*[\s\S]*?\*\//.exec(rest)
    if (blockComment) {
      masked += " ".repeat(blockComment[0].length)
      index += blockComment[0].length
      continue
    }
    const lineComment = /^\/\/[^\n]*/.exec(rest)
    if (lineComment) {
      masked += " ".repeat(lineComment[0].length)
      index += lineComment[0].length
      continue
    }
    const text = /^(["'`])(?:\\.|(?!\1)[\s\S])*\1/.exec(rest)
    if (text) {
      masked += " ".repeat(text[0].length)
      index += text[0].length
      continue
    }
    masked += source[index]
    index += 1
  }
  return masked
}

/**
 * The element names directly under `<div className="nessa-composer">`.
 *
 * Depth is counted over tag tokens, so a notice handed to `ComposerNotices` as
 * a prop is inside that element rather than beside it — which is exactly the
 * distinction the rule is about. Returns null when the element is not found,
 * so a rename fails loudly instead of passing on an empty list.
 */
export function composerChildren(source) {
  // Masking blanks each character it removes rather than dropping it, so an
  // index found in the source still points at the same place in the mask. The
  // class name is only findable in the source: as a string, it is blanked.
  const code = maskJsxNonCode(source)
  const opening = source.indexOf('className="nessa-composer"')
  if (opening === -1) return null
  const tags = /<([A-Za-z][\w.]*)|<\/[A-Za-z][\w.]*\s*>|\/>/g
  tags.lastIndex = source.indexOf(">", opening) + 1
  const children = []
  let depth = 1
  for (let tag = tags.exec(code); tag; tag = tags.exec(code)) {
    if (tag[1] === undefined) {
      depth -= 1
      if (depth === 0) return children
      continue
    }
    if (depth === 1) children.push(tag[1])
    depth += 1
  }
  return children
}

/**
 * What may sit directly in the composer, in order.
 *
 * Everything said goes in the box. The queue badge and the delivery row are
 * outside it on purpose — they are controls rather than statements, one short
 * row each, and a control the composer is about to obey must not be somewhere
 * you have to scroll to find. That makes them part of the budget's fixed cost,
 * which is why the list is exact rather than a set of allowed names: a third
 * pinned row is a change to the budget and should be argued for here.
 */
const composerLayout = [
  "ComposerNotices",
  "ConversationQueue",
  "ComposerDeliveryMode",
  "PillComposer",
]

export function composerNoticeViolations(path, source) {
  if (path !== "src/panel/ui/app.tsx") return []
  const children = composerChildren(source)
  if (children === null) {
    return [
      'the composer element no longer carries className="nessa-composer"; move this check with it',
    ]
  }
  if (children.join(" ") === composerLayout.join(" ")) return []
  return [
    `the composer holds ${children.join(", ") || "nothing"}; it holds exactly ${composerLayout.join(", ")}, because everything said above the pill goes through ComposerNotices, which owns the room the notices may have and the order they are said in`,
  ]
}

/** How many times the panel's height the notices may be, at most. A third. */
export const noticeShare = 3

/**
 * The declared ceiling on the notices, as a function of the panel's height —
 * or null when the stylesheet no longer declares one.
 *
 * The panel's height arrives from the host as `--nessa-window-height` (see
 * `panel-frame.ts`), so a cap written against it is a cap that holds at every
 * size the window can be, rather than one number that is right at 900px and
 * useless at 320px.
 */
export function declaredNoticeCap(styles) {
  const rule = /\.nessa-composer-notices\s*\{([^}]*)\}/.exec(styles)
  if (!rule) return null
  const cap =
    /max-height:\s*calc\(\s*var\(\s*--nessa-window-height[^)]*\)\s*\/\s*([\d.]+)\s*\)/.exec(
      rule[1],
    )
  if (!cap) return null
  const divisor = Number.parseFloat(cap[1])
  if (!Number.isFinite(divisor) || divisor <= 0) return null
  return (windowHeight) => windowHeight / divisor
}

export function composerBudgetViolations(styles) {
  const failures = []
  const rule = /\.nessa-composer-notices\s*\{([^}]*)\}/.exec(styles)
  if (!rule) {
    return [
      ".nessa-composer-notices has no rule; the notices above the pill are unbounded again and a short panel loses the composer",
    ]
  }
  const cap = declaredNoticeCap(styles)
  if (!cap) {
    failures.push(
      "the notices need a max-height derived from --nessa-window-height; a fixed number is wrong at one of 320px and 900px, and no number at all pushes the pill out of the window",
    )
  } else if (cap(1) > 1 / noticeShare) {
    failures.push(
      `the notices may take at most a third of the panel; the stylesheet gives them ${(cap(1) * 100).toFixed(0)}%, which leaves the transcript and the pill too little at the window's minimum height`,
    )
  }
  if (!/overflow-y:\s*(auto|scroll)/.test(rule[1])) {
    failures.push(
      "the notices must scroll under their ceiling; without overflow-y the cap clips a card and takes its Retry off the screen for good",
    )
  }
  if (!/\.nessa-composer-notices:empty\s*\{[^}]*display:\s*none/.test(styles)) {
    failures.push(
      "an empty notice strip must be display:none, or a silent composer keeps a tab stop and a gutter nobody asked for",
    )
  }
  return failures
}
