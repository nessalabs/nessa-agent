/** Test-only gateway composition. The real admission and VM task runner use a
 * fake workspace provider and the actual helper binary, without Boat or SSH. */
import { spawn } from "node:child_process"
import { chmodSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { repoRoot } from "./server.mjs"

const quote = (value) => `'${value.replaceAll("'", "'\\''")}'`

export async function vmBackgroundFixture(directory, idleSeconds) {
  if (!["linux", "darwin"].includes(process.platform))
    throw new Error("the VM background gateway fixture requires a Unix gateway")
  const root = join(directory, "vm-background")
  mkdirSync(root, { recursive: true })
  const testBinary = await new Promise((resolve, reject) => {
    const child = spawn(
      "cargo",
      ["test", "-p", "nessa-server", "--lib", "--no-run", "--message-format=json"],
      { cwd: repoRoot, stdio: ["ignore", "pipe", "pipe"] },
    )
    let output = ""
    let diagnostics = ""
    child.stdout.on("data", (chunk) => (output += chunk))
    child.stderr.on("data", (chunk) => (diagnostics = (diagnostics + chunk).slice(-8000)))
    child.on("error", reject)
    child.on("close", (code) => {
      if (code !== 0) return reject(new Error(`VM fixture build failed: ${diagnostics}`))
      const artifacts = output
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line))
      const test = artifacts.find(
        (item) =>
          item.reason === "compiler-artifact" &&
          item.target.name === "nessa_server" &&
          item.profile.test &&
          item.executable,
      )
      if (!test) return reject(new Error("VM fixture build returned no test executable"))
      resolve(test.executable)
    })
  })
  const cli = process.env.MCP_LIVE_NESSA ?? join(repoRoot, "target/debug/nessa")
  const executable = join(root, "gateway")
  writeFileSync(
    executable,
    `#!/bin/sh\nif [ "$1" = server ]; then\n  exec ${quote(testBinary)} --exact composition::vm_background_gateway::serve --ignored --nocapture\nfi\nexec ${quote(cli)} "$@"\n`,
  )
  chmodSync(executable, 0o700)
  return {
    executable,
    root,
    environment: {
      NESSA_VM_STRESS_ROOT: root,
      NESSA_VM_STRESS_CLI: cli,
      NESSA_VM_STRESS_IDLE_SECONDS: String(idleSeconds),
    },
    events: () => vmBackgroundEvents(root),
  }
}

/** Ignore an append still in progress; malformed complete records remain errors. */
export function vmBackgroundEvents(root) {
  try {
    const lines = readFileSync(join(root, "events.jsonl"), "utf8").split("\n")
    lines.pop()
    return lines.map((line) => JSON.parse(line))
  } catch (error) {
    if (error.code === "ENOENT") return []
    throw error
  }
}
