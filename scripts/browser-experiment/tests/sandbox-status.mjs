// Current Chromium chrome://sandbox table contract, inspected 2026-10-23:
// chrome/browser/resources/sandbox_internals/sandbox_internals.ts
export function adequatelySandboxed(rows, evaluation) {
  const table = new Map(rows)
  return (
    ["Namespace", "SUID"].includes(table.get("Layer 1 Sandbox")) &&
    table.get("PID namespaces") === "Yes" &&
    table.get("Seccomp-BPF sandbox") === "Yes" &&
    evaluation === "You are adequately sandboxed."
  )
}
