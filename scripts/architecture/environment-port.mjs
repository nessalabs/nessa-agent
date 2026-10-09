import { normalizedPath } from "./rust-boundaries.mjs"

/** The one file that may implement the port in product source. */
const ADAPTER = "crates/nessa-server/src/conversation/infrastructure/environment.rs"

/** Where product source may name the port: the service, its adapter, and composition. */
const HOLDERS = [
  "crates/nessa-server/src/conversation/application/",
  "crates/nessa-server/src/conversation/infrastructure/",
  "crates/nessa-server/src/composition/",
]

const IMPLEMENTS = /\bimpl\s+Environment\s+for\b/
const NAMES =
  /\bdyn\s+Environment\b|\bEnvironmentDeclaration\b|\bEnvironmentFuture\b|\bin_process_environment\b|\bimpl\s+Environment\s+for\b/

/** Rust source without its line comments, so a doc sentence naming the port is not a use of it. */
function code(source) {
  return source.replace(/\/\/.*$/gm, "")
}

/**
 * The Environment port (ADR 252) sits below the conversation service's Agent,
 * and has one adapter in product source: the in-process one. A surface — a
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
  if (IMPLEMENTS.test(text) && file !== ADAPTER)
    violations.push(
      `the Environment port has one adapter in product source, ${ADAPTER}; implement another only with the slice that adds its transport`,
    )
  else if (NAMES.test(text) && !HOLDERS.some((holder) => file.startsWith(holder)))
    violations.push(
      "only the conversation service, its environment adapter and composition may name the Environment port; a surface goes through the conversation service",
    )
  return violations
}
