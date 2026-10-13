import { maskRustNonCode, normalizedPath } from "./rust-boundaries.mjs"

/**
 * Policy goes through Cedar (CODING_STANDARDS.md gate 18, ADR 803).
 *
 * Whether someone may do something to something is a Cedar policy, behind a
 * port the application owns, implemented by an adapter in a module directory
 * named `cedar`. This holds the part a text check can see: where the engine
 * is. It refuses the `cedar_policy` crate outside a `cedar` directory, so the
 * engine cannot leak past its adapter, and it refuses an implementation of a
 * `…PolicyEvaluator` port outside one, so a port meant for Cedar cannot be
 * answered by hand-written Rust instead.
 *
 * It cannot tell a hand-written policy decision — a role compared, a grant
 * list scanned — from any other Rust, and does not pretend to; review holds
 * that half of the gate.
 *
 * Test code may substitute a port, so `#[cfg(test)]` modules, `tests.rs` and
 * `*_tests.rs` files, and anything outside `src/` are not read. Pure text on
 * purpose: `check-architecture.mjs` runs with bare Node.
 */

/** A path segment named `cedar` is an adapter directory. */
const ADAPTER = /(?:^|\/)cedar\//

/** A name of the `cedar_policy` crate in code: a path, an import, an extern. */
const ENGINE = /\bcedar_policy\b/

/**
 * An implementation of a policy port, generic or through a qualified path:
 * `impl PolicyEvaluator for X`, `impl<T> crate::ports::ToolPolicyEvaluator for X`.
 */
const IMPLEMENTS = /\bimpl\s*(?:<[^{;]*?>\s*)?(?:[\w:]*::)?\w*PolicyEvaluator\s+for\b/

/** `text` with each `#[cfg(test)]` module's body blanked, lines kept. */
function withoutTestModules(text) {
  const out = text.split("")
  const opening = /#\[cfg\(test\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{/g
  for (let match; (match = opening.exec(text)); ) {
    let depth = 1
    let index = match.index + match[0].length
    for (; index < text.length && depth > 0; index += 1) {
      if (text[index] === "{") depth += 1
      else if (text[index] === "}") depth -= 1
    }
    for (let blank = match.index; blank < index; blank += 1)
      if (out[blank] !== "\n") out[blank] = " "
    opening.lastIndex = index
  }
  return out.join("")
}

/** Whether `file` is test code by its name or place. */
function isTestFile(file) {
  return !file.includes("/src/") || /(?:^|\/)tests\.rs$|_tests\.rs$|\/tests\//.test(file)
}

/** Violations of where the policy engine may live, for one Rust file. */
export function policyEngineViolations(path, source) {
  const file = normalizedPath(path)
  if (isTestFile(file) || ADAPTER.test(file)) return []
  const text = withoutTestModules(maskRustNonCode(source))
  const violations = []
  if (ENGINE.test(text))
    violations.push(
      "the cedar_policy crate is named only inside a `cedar` adapter directory; the rest of the code asks its policy port (CODING_STANDARDS.md#policy-goes-through-cedar)",
    )
  if (IMPLEMENTS.test(text))
    violations.push(
      "a policy port is implemented by a Cedar adapter in a `cedar` directory, not by hand-written Rust (CODING_STANDARDS.md#policy-goes-through-cedar)",
    )
  return violations
}
