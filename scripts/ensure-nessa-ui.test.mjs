import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { ensureWorkspace, linkedWorkspacePackages } from "./ensure-nessa-ui.mjs"

test("clean vendor preparation installs and builds every linked source package", (context) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-linked-workspace-"))
  context.after(() => rmSync(root, { recursive: true, force: true }))
  const manifests = [
    ["packages/react", "@nessa-ui/react", undefined],
    ["packages/agent-stream", "@nessalabs/agent-stream", "tsup"],
  ]
  for (const [directory, name, build] of manifests) {
    mkdirSync(join(root, directory), { recursive: true })
    writeFileSync(
      join(root, directory, "package.json"),
      JSON.stringify({ name, scripts: build ? { build } : {} }),
    )
  }
  const calls = []
  ensureWorkspace(root, (command, args, cwd) => calls.push({ command, args, cwd }))
  assert.deepEqual(
    linkedWorkspacePackages,
    manifests.map(([directory]) => directory),
  )
  assert.deepEqual(calls, [
    {
      command: "pnpm",
      args: [
        "install",
        "--frozen-lockfile",
        "--filter",
        "@nessa-ui/react...",
        "--filter",
        "@nessalabs/agent-stream...",
      ],
      cwd: root,
    },
    {
      command: "pnpm",
      args: ["--filter", "@nessalabs/agent-stream", "build"],
      cwd: root,
    },
  ])
})
