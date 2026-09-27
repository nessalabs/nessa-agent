/**
 * `in` is not an existence check.
 *
 * `key in table` walks the prototype chain, so it is true for every name
 * `Object.prototype` carries — `constructor`, `toString`, `valueOf`,
 * `__proto__` — none of which any table put there. That is how
 * `NESSA_STAGE=toString` passed the guard in `src/env/gateway-ports.ts` and
 * came back out as `http://127.0.0.1:function toString() { [native code] }`.
 * `Object.hasOwn` asks the question the guard meant to ask.
 *
 * The rule is deliberately narrower than the one #110 wished for. "No computed
 * member access with a key typed `string`" needs type information, and with it
 * the rule is still wrong in both directions here: it misses `gateway-ports.ts`
 * entirely, because a cast had already laundered that key into the `Stage`
 * union, and it fires on nine correct lines that walk a parsed-JSON bag with
 * their own literal keys. This shape needs no types, has no exceptions in this
 * repository, and is the one the flagship bug was made of.
 *
 * A literal on the left is the safe direction and is allowed: there the object
 * is the untrusted thing and the key is ours, which is how every `in` in
 * `packages/nessa-client/src/protocol/validate.ts` reads. `#brand in object` is
 * a private-field check and cannot reach a prototype member at all.
 *
 * Scope is the config's: `eslint.config.js` turns this on for `src` and
 * `packages`. ESLint does not lint `scripts`, so the twin of this rule's
 * flagship case — `scripts/gateway-port.mjs`, which reads the same environment
 * variable against the same table — is out of its reach and is covered by
 * `scripts/gateway-port.test.mjs` instead.
 */
export const inheritedLookups = {
  meta: {
    type: "problem",
    docs: {
      description:
        "Ask what a table owns with Object.hasOwn rather than what it inherits with `in`",
    },
    schema: [],
    messages: {
      inherited:
        "`{{key}} in {{table}}` is true for every name `Object.prototype` carries — `constructor`, `toString`, `__proto__` — so a key from outside this process passes a guard written this way and reads a function out of the table. Ask `Object.hasOwn({{table}}, {{key}})` instead, or narrow the key into a union before it gets here.",
    },
  },
  create(context) {
    const sourceCode = context.sourceCode
    /** A key written out here is one you can read; it is the object that is
     * untrusted in that direction, not the name. */
    const written = (key) =>
      key.type === "Literal" ||
      key.type === "PrivateIdentifier" ||
      (key.type === "TemplateLiteral" && key.expressions.length === 0)

    return {
      BinaryExpression(node) {
        if (node.operator !== "in" || written(node.left)) return
        context.report({
          node,
          messageId: "inherited",
          data: {
            key: sourceCode.getText(node.left),
            table: sourceCode.getText(node.right),
          },
        })
      },
    }
  },
}
