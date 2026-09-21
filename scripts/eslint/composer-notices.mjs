/**
 * Everything the composer says above the pill goes through one box.
 *
 * Five independent producers write to that strip and none of them excludes the
 * others. For a long time the whole arrangement was JSX order in `app.tsx`:
 * nothing bounded how tall the column got, nothing decided which card was read
 * first, and on a short panel the pill was pushed out of the window.
 * `ComposerNotices` owns both now — the room the notices may have and the order
 * they are said in — and this rule is what keeps a sixth notice from being
 * pasted in beside it and quietly taking neither.
 *
 * It is a lint rule rather than a check in `scripts/architecture` because of
 * where it has to run. That script runs on the Rust jobs too, with bare Node
 * and no `node_modules`, so a rule there may not import a parser; the first
 * version of this counted angle brackets instead and an ordinary `<>…</>`
 * disarmed it. ESLint already parses this file properly, already runs where the
 * dependencies are, and points at the offending node rather than a line number.
 * The stylesheet's half of the budget is pure text and stays in that script.
 *
 * Scope is the config's: `eslint.config.js` turns this on for the panel's
 * chrome and nothing else. The rule is about that one file's composition.
 */

/** The four things that may sit directly in the composer, in order. */
const composerLayout = [
  "ComposerNotices",
  "ConversationQueue",
  "ComposerDeliveryMode",
  "PillComposer",
]

const isElement = (node) => node.type === "JSXElement" || node.type === "JSXFragment"

export const composerNotices = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Keep every notice the composer says inside ComposerNotices, which owns the room they may take and the order they are said in",
    },
    schema: [],
    messages: {
      layout:
        "The composer holds {{actual}}. It holds exactly {{expected}}: everything said above the pill goes through ComposerNotices, which bounds the column and decides the order. The queue badge and the delivery row are outside it on purpose — they are controls rather than statements, one short row each, and a control the composer is about to obey must not be somewhere you have to scroll to find.",
      outside:
        "This notice is outside ComposerNotices. Every notice the chrome says goes through the box, which is what bounds the column above the pill and what decides which card is read first.",
      missing:
        'The chrome no longer has an element with className="nessa-composer". Move this rule with it.',
    },
  },
  create(context) {
    const sourceCode = context.sourceCode
    const visitorKeys = sourceCode.visitorKeys
    let composer = null

    const tagOf = (node) =>
      node.type === "JSXFragment" ? "<>" : sourceCode.getText(node.openingElement.name)

    const isComposer = (node) =>
      node.type === "JSXElement" &&
      node.openingElement.attributes.some(
        (attribute) =>
          attribute.type === "JSXAttribute" &&
          attribute.name.name === "className" &&
          attribute.value?.type === "Literal" &&
          attribute.value.value === "nessa-composer",
      )

    /**
     * What a JSX child position actually renders.
     *
     * `{generating && <ComposerDeliveryMode />}` renders one element through an
     * expression, and an expression holding nothing but a comment renders
     * nothing, so the expression is walked — but only as far as the first
     * element in it. Anything below that is that element's own business, which
     * is exactly how a notice handed to `ComposerNotices` as a prop is inside
     * the box rather than beside it.
     */
    const rendered = (node, found) => {
      if (isElement(node)) {
        found.push(tagOf(node))
        return found
      }
      if (node.type === "JSXText") {
        if (node.value.trim() !== "") found.push("text")
        return found
      }
      for (const key of visitorKeys[node.type] ?? []) {
        const value = node[key]
        if (Array.isArray(value)) {
          for (const child of value) if (child?.type) rendered(child, found)
        } else if (value?.type) rendered(value, found)
      }
      return found
    }

    /**
     * Whether a notice is inside the box, wherever it was written.
     *
     * The ancestor walk passes through attributes, so a notice handed to
     * `ComposerNotices` as a prop counts as inside — and the same notice handed
     * to a banner prop on the queue counts as outside, which the list of direct
     * children alone cannot see.
     */
    const insideTheBox = (node) => {
      for (let parent = node.parent; parent; parent = parent.parent) {
        if (isElement(parent) && tagOf(parent) === "ComposerNotices") return true
      }
      return false
    }

    return {
      JSXElement(node) {
        if (isComposer(node)) {
          composer = node
          const children = node.children.reduce(
            (found, child) => rendered(child, found),
            [],
          )
          if (children.join(" ") !== composerLayout.join(" ")) {
            context.report({
              node: node.openingElement,
              messageId: "layout",
              data: {
                actual: children.join(", ") || "nothing",
                expected: composerLayout.join(", "),
              },
            })
          }
        }
        if (tagOf(node) === "AgentNotification" && !insideTheBox(node)) {
          context.report({ node, messageId: "outside" })
        }
      },
      "Program:exit"(node) {
        if (!composer) context.report({ node, messageId: "missing" })
      },
    }
  },
}

/** The plugin `eslint.config.js` registers. One rule, about one file. */
export default { rules: { "composer-notices": composerNotices } }
