import { posix } from "node:path"
import { maskRustNonCode, normalizedPath } from "./rust-boundaries.mjs"

/**
 * Policy goes through Cedar (CODING_STANDARDS.md gate 18, ADR 803).
 *
 * Whether someone may do something to something is a Cedar policy, behind a
 * port the application owns, named `…PolicyEvaluator`, and implemented by an
 * adapter in a module directory named `cedar`. This holds the part a text
 * check can see: where the engine is. It refuses the `cedar_policy` crate
 * anywhere in a crate's `src/` outside a `cedar` directory, test modules
 * included, so the engine
 * cannot leak past its adapter. It refuses an implementation of a
 * `…PolicyEvaluator` port outside one, so a port meant for Cedar cannot be
 * answered by hand-written Rust instead; only test code may substitute it.
 *
 * It cannot tell a hand-written policy decision — a role compared, a grant
 * list scanned — from any other Rust, and does not pretend to; review holds
 * that half of the gate.
 *
 * Test code is what the compiler says it is: the body of a `#[cfg(test)]`
 * module, and the files an out-of-line `#[cfg(test)] mod name;` loads and the
 * modules they declare in turn. [`testCodeFiles`] follows those declarations
 * the way rustc resolves a file's top-level modules, so no file name is taken
 * as a sign of test code. Two things it does not follow, and reads as product
 * code: a `mod name;` nested inside an inline `mod … { }` block, and a file
 * pulled in with `include!`. Only a crate's `src/` is read; tests, benches and
 * examples outside it are not. Pure text on purpose: `check-architecture.mjs`
 * runs with bare Node.
 */

/** A path segment named `cedar` is an adapter directory. */
const ADAPTER = /(?:^|\/)cedar\//

/** A name of the `cedar_policy` crate in code: a path, an import, an extern. */
const ENGINE = /\bcedar_policy\b/

/**
 * An implementation of a policy port, generic or through a qualified path:
 * `impl PolicyEvaluator for X`, `impl<T> ports::ToolPolicyEvaluator<T> for X`.
 */
const IMPLEMENTS =
  /\bimpl\s*(?:<[^{;]*?>\s*)?(?:[\w:]*::)?\w*PolicyEvaluator\s*(?:<[^{;]*?>\s*)?\s+for\b/

/** `#[cfg(test)]`, or a `cfg(all(test, …))`. */
const TEST_CFG = String.raw`#\[cfg\((?:test|all\(\s*test\b[^\]]*)\)\]`

/** A test cfg, then any further attributes. */
const TEST_ATTRIBUTES = String.raw`${TEST_CFG}\s*((?:#\[[^\]]*\]\s*)*)`

/** The `mod` keyword after a visibility, if any. */
const MOD = String.raw`(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*`

/** `text` with each inline `#[cfg(test)]` module's body blanked, lines kept. */
function withoutTestModules(text) {
  const out = text.split("")
  const opening = new RegExp(`${TEST_ATTRIBUTES}${MOD}\\{`, "g")
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

/** Any out-of-line `mod name;`, its attributes captured. */
const DECLARATION = new RegExp(String.raw`((?:#\[[^\]]*\]\s*)*)${MOD};`, "g")

/** Whether a declaration's attributes put it behind a test cfg. */
const IS_TEST = new RegExp(TEST_CFG)

/**
 * The out-of-line modules `path` declares, as the files rustc would load for
 * a declaration at the top level of the file (not inside an inline module):
 * `name.rs` or `name/mod.rs` in the directory `path` owns, or a `#[path]`
 * relative to `path`'s own directory. A `mod.rs`, `lib.rs` or `main.rs` owns
 * its directory, and so does a file loaded through `#[path]`
 * (`ownsDirectory`); any other `foo.rs` owns `foo/`. `testOnly` keeps only
 * `cfg(test)` declarations. Paths are repository-relative.
 */
export function moduleDeclarations(
  path,
  source,
  { ownsDirectory = false, testOnly = false } = {},
) {
  const file = normalizedPath(path)
  const masked = maskRustNonCode(source)
  const dir = posix.dirname(file)
  const stem = posix.basename(file, ".rs")
  const owns =
    ownsDirectory || ["mod", "lib", "main"].includes(stem) ? dir : posix.join(dir, stem)
  const modules = []
  for (const match of source.matchAll(DECLARATION)) {
    const at = match.index + match[0].search(/\S/)
    if (masked[at] !== source[at]) continue
    if (testOnly && !IS_TEST.test(match[1])) continue
    const pathAttribute = /#\[path\s*=\s*"([^"]+)"\s*\]/.exec(match[1])
    modules.push(
      pathAttribute
        ? [{ path: posix.normalize(posix.join(dir, pathAttribute[1])), byPath: true }]
        : [
            { path: posix.join(owns, `${match[2]}.rs`), byPath: false },
            { path: posix.join(owns, match[2], "mod.rs"), byPath: false },
          ],
    )
  }
  return modules
}

/**
 * Every file under `sources` (a map of repository-relative path to text) that
 * only test code loads: what a `cfg(test)` declaration loads, and every module
 * those files declare in turn.
 */
export function testCodeFiles(sources) {
  const tests = new Set()
  const queue = []
  const load = (modules) => {
    for (const candidates of modules)
      for (const candidate of candidates)
        if (sources.has(candidate.path) && !tests.has(candidate.path)) {
          tests.add(candidate.path)
          queue.push(candidate)
        }
  }
  for (const [path, source] of sources)
    load(moduleDeclarations(path, source, { testOnly: true }))
  while (queue.length) {
    const next = queue.shift()
    load(
      moduleDeclarations(next.path, sources.get(next.path), {
        ownsDirectory: next.byPath,
      }),
    )
  }
  return tests
}

/**
 * Violations of where the policy engine may live, for one Rust file under a
 * crate's `src/`. `test` says the whole file is test code.
 */
export function policyEngineViolations(path, source, test = false) {
  const file = normalizedPath(path)
  if (ADAPTER.test(file)) return []
  const text = maskRustNonCode(source)
  const violations = []
  if (ENGINE.test(text))
    violations.push(
      "the cedar_policy crate is named only inside a `cedar` adapter directory; the rest of the code asks its policy port (CODING_STANDARDS.md#policy-goes-through-cedar)",
    )
  if (!test && IMPLEMENTS.test(withoutTestModules(text)))
    violations.push(
      "a policy port is implemented by a Cedar adapter in a `cedar` directory, not by hand-written Rust (CODING_STANDARDS.md#policy-goes-through-cedar)",
    )
  return violations
}
