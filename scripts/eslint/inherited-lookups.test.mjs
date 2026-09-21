/**
 * These run under `pnpm lint:rules`, in the frontend job, beside the composer's
 * own rule tests and for the same reason: a rule tester needs ESLint itself.
 */
import test from "node:test"
import { RuleTester } from "eslint"
import tseslint from "typescript-eslint"

import { inheritedLookups } from "./inherited-lookups.mjs"

const tester = new RuleTester({
  languageOptions: {
    parser: tseslint.parser,
    ecmaVersion: "latest",
    sourceType: "module",
  },
})

const run = (cases) => tester.run("inherited-lookups", inheritedLookups, cases)

test("a key from outside cannot be looked up with `in`", () => {
  run({
    valid: [],
    invalid: [
      // The shape #110 was reported for, as it stood.
      {
        code: `
          const stages = table.stages
          export function parseStage(value) {
            const named = value.trim()
            if (!(named in stages)) throw new Error("not a stage")
            return named
          }
        `,
        errors: [{ messageId: "inherited" }],
      },
      // A member expression on the left is no more readable than a variable.
      {
        code: `if (entry.id in agents) use(agents[entry.id])`,
        errors: [{ messageId: "inherited" }],
      },
      // A template that interpolates is a computed name like any other.
      {
        code: "if (`${prefix}-id` in table) use(table)",
        errors: [{ messageId: "inherited" }],
      },
    ],
  })
})

test("the name itself names the offending pair, so the fix is obvious", () => {
  run({
    valid: [],
    invalid: [
      {
        code: `if (named in stages) use(named)`,
        errors: [
          {
            message:
              "`named in stages` is true for every name `Object.prototype` carries — `constructor`, `toString`, `__proto__` — so a key from outside this process passes a guard written this way and reads a function out of the table. Ask `Object.hasOwn(stages, named)` instead, or narrow the key into a union before it gets here.",
          },
        ],
      },
    ],
  })
})

test("a name written out here is ours, and the object is the untrusted one", () => {
  run({
    valid: [
      // How every `in` in the client's protocol validators reads: a literal
      // name against a frame that arrived from the wire.
      `if ("error" in frame && typeof frame.error === "object") reject(frame.error)`,
      `if (0 in list) use(list[0])`,
      "if (`error` in frame) reject(frame)",
      // A private-field brand check cannot reach a prototype member.
      `class Lease { static holds(value) { return #token in value } }`,
    ],
    invalid: [],
  })
})

test("`for (const key in table)` is iteration, not a lookup", () => {
  run({
    valid: [`for (const key in table) use(table[key])`],
    invalid: [],
  })
})

test("Object.hasOwn is the shape the rule is asking for", () => {
  run({
    valid: [
      `
        const stages = table.stages
        export function parseStage(value) {
          const named = value.trim()
          if (!Object.hasOwn(stages, named)) throw new Error("not a stage")
          return named
        }
      `,
    ],
    invalid: [],
  })
})
