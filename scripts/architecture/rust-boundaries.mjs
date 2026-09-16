// Source-level dependency guard, complementary to compilation and human review.
// Checks explicit Rust imports; it does not resolve macros or the full module graph.
export function rustBoundaryViolations(path, source) {
  const domain = /(?:\/domain\/|\/domain\.rs$)/.test(path)
  const application = /(?:\/application\/|\/application\.rs$)/.test(path)
  if (!domain && !application) return []
  // Documentation examples may deliberately illustrate composition. Only imports
  // in source participate; test fixtures live outside these production trees.
  const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "")
  const imports = [...code.matchAll(/\buse\s+([^;]+);/g)].map((match) => match[1])
  const failures = new Set()
  for (const item of imports) {
    if (/\b(infrastructure|composition|product|adapters|presentation)\b/.test(item)) {
      failures.add(
        "domain/application imports must point inward, through application-owned ports",
      )
    }
    if (domain && /\bapplication\b/.test(item)) {
      failures.add("domain must not import application DTOs or orchestration")
    }
    if (domain && /\b(tokio|tauri|serde|serde_json|reqwest|axum|shepherd)\b/.test(item)) {
      failures.add(
        "domain must not import runtime, transport, or serialization libraries",
      )
    }
    if (domain && /\bstd\b[\s\S]*\b(fs|net|process)\b/.test(item)) {
      failures.add("domain must not import filesystem, network, or process effects")
    }
  }
  return [...failures]
}
