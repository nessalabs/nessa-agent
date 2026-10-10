import { normalizedPath } from "./rust-boundaries.mjs"

/** The files that may implement the port in product source: the in-process
 * adapter, and the SSH one (issue #699). */
const ADAPTERS = [
  "crates/nessa-server/src/conversation/infrastructure/environment.rs",
  "crates/nessa-server/src/conversation/infrastructure/ssh_environment/environment.rs",
]

/** Where product source may name the port: the service, its adapter, and composition. */
const HOLDERS = [
  "crates/nessa-server/src/conversation/application/",
  "crates/nessa-server/src/conversation/infrastructure/",
  "crates/nessa-server/src/composition/",
]

/** The port's own names. `Environment` alone is not one: the configuration
 * has an `Environment` of its own, so the bare word is the port only where a
 * path or an import says so. */
const PORT = String.raw`(?:Environment|EnvironmentDeclaration|EnvironmentFuture)`

/** An implementation of the port, generic or by a qualified path. */
const IMPLEMENTS = new RegExp(
  String.raw`\bimpl\s*(?:<[^{;]*?>\s*)?(?:[\w:]*::)?Environment\s+for\b`,
)

/**
 * A use of the port. Any use outside the service names it through its path —
 * `conversation::application`, or that module's `environment` — whether as an
 * import (`use …::application::Environment`, `use …::application::{…,
 * Environment}`, a glob of either), or written out in place (`dyn
 * crate::conversation::application::Environment`, a bound `E:
 * application::Environment`). Matching the path is what catches every shape
 * of use: a bare `Environment` in a bound, a `dyn`, or an `impl` needs one.
 * The path is the conversation module's, relative (`super::application`) or
 * not, or `application` itself once that module is imported; another
 * crate's `…::application` is not it.
 */
const NAMES = new RegExp(
  [
    String.raw`(?:\bconversation::|\b(?:self|super)::(?:super::)*|(?<!::)\b)application::(?:environment::)?(?:${PORT}\b|\*|\{[^}]*?(?:\b${PORT}\b|\*))`,
    String.raw`\bEnvironmentDeclaration\b`,
    String.raw`\bEnvironmentFuture\b`,
    String.raw`\bin_process_environment\b`,
    String.raw`\bdyn\s+(?:[\w:]*::)?Environment\b`,
    IMPLEMENTS.source,
  ].join("|"),
)

/** Rust source without its line comments, so a doc sentence naming the port is not a use of it. */
function code(source) {
  return source.replace(/\/\/.*$/gm, "")
}

/**
 * The Environment port (ADR 252) sits below the conversation service's Agent,
 * and has two adapters in product source: the in-process one and the SSH one. A surface — a
 * product method, the protocol's view, the desktop runtime — that could reach
 * the port could pick where an agent runs without the lease that says so, so
 * only the service, the adapter and composition name it. Tests are outside
 * `src/` and may hold substitutes.
 */
export function environmentPortViolations(path, source) {
  const file = normalizedPath(path)
  if (!file.startsWith("crates/") || !file.includes("/src/")) return []
  const text = code(source)
  const violations = []
  if (IMPLEMENTS.test(text) && !ADAPTERS.includes(file))
    violations.push(
      `the Environment port's adapters in product source are ${ADAPTERS.join(" and ")}; implement another only with the slice that adds its transport`,
    )
  else if (NAMES.test(text) && !HOLDERS.some((holder) => file.startsWith(holder)))
    violations.push(
      "only the conversation service, its environment adapters and composition may name the Environment port; a surface goes through the conversation service",
    )
  return violations
}
