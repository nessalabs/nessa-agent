/**
 * Resolve exactly the executable artifacts produced by one Cargo invocation.
 *
 * Cargo owns target-directory and target-triple placement. Reading its artifact
 * messages avoids reconstructing either rule from environment and config.
 */
export function builtExecutables(output) {
  const executables = new Map()
  for (const line of output.split("\n")) {
    if (!line.startsWith("{")) continue
    const message = JSON.parse(line)
    if (
      message.reason === "compiler-artifact" &&
      typeof message.target?.name === "string" &&
      typeof message.executable === "string"
    )
      executables.set(message.target.name, message.executable)
  }
  return executables
}
