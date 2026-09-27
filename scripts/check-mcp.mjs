import { spawnSync } from "node:child_process"

if (process.platform === "win32") {
  console.log(
    "nessa-mcp native process supervision is unsupported on Windows; check skipped",
  )
  process.exit(0)
}

for (const args of [
  ["fmt", "-p", "nessa-mcp", "--", "--check"],
  ["clippy", "-p", "nessa-mcp", "--all-targets", "--", "-D", "warnings"],
  ["test", "-p", "nessa-mcp"],
]) {
  const result = spawnSync("cargo", args, { stdio: "inherit" })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)
}
