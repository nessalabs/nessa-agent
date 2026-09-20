import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import {
  checkVendor,
  ensureWorkspace,
  linkedWorkspacePackages,
  prepareVendor,
} from "./ensure-nessa-ui.mjs"

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

const PIN = "a".repeat(40)
const OTHER = "b".repeat(40)

/** A repo root with a pin and, optionally, a vendor that looks like a workspace. */
function makeRoot(context, { vendor = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), "nessa-vendor-"))
  context.after(() => rmSync(root, { recursive: true, force: true }))
  writeFileSync(join(root, "nessa-ui-revision"), `${PIN}\n`)
  if (vendor) {
    for (const directory of linkedWorkspacePackages) {
      mkdirSync(join(root, ".vendor/nessa_ui", directory), { recursive: true })
      writeFileSync(
        join(root, ".vendor/nessa_ui", directory, "package.json"),
        JSON.stringify({ name: directory }),
      )
    }
  }
  return root
}

/** Fake git: answers rev-parse with `head` and status with `status`. */
function fakeGit({ head, status = "" }) {
  return (args) => {
    if (args[0] === "rev-parse") return head
    if (args[0] === "status") return status
    return null
  }
}

test("check passes when the vendor is at the pin", (context) => {
  const root = makeRoot(context)
  assert.equal(checkVendor({ root, git: fakeGit({ head: PIN }) }), null)
})

test("check names both commits and the remedy when the vendor is behind", (context) => {
  const root = makeRoot(context)
  const problem = checkVendor({ root, git: fakeGit({ head: OTHER }) })
  assert.match(problem, /at bbbbbbb but nessa-ui-revision wants aaaaaaa/)
  assert.match(problem, /pnpm ui:types/)
})

test("check tells you to install when the vendor is missing", (context) => {
  const root = makeRoot(context, { vendor: false })
  const problem = checkVendor({ root, git: fakeGit({ head: PIN }) })
  assert.match(problem, /missing/)
  assert.match(problem, /pnpm ui:types/)
})

test("prepare advances a clean clone to the pin on its own", (context) => {
  const root = makeRoot(context)
  const calls = []
  prepareVendor({
    root,
    git: fakeGit({ head: OTHER }),
    run: (command, args) => calls.push([command, ...args]),
  })
  assert.deepEqual(calls.slice(0, 2), [
    ["git", "fetch", "--depth", "1", "origin", PIN],
    ["git", "checkout", "--detach", PIN],
  ])
  assert.equal(calls[2][1], "install")
})

test("prepare leaves a clone with local edits alone", (context) => {
  const root = makeRoot(context)
  const calls = []
  assert.throws(
    () =>
      prepareVendor({
        root,
        git: fakeGit({ head: OTHER, status: " M packages/react/src/x.ts" }),
        run: (command, args) => calls.push([command, ...args]),
      }),
    /Preserving changes/,
  )
  assert.deepEqual(calls, [])
})

test("prepare never moves a symlinked sibling checkout", (context) => {
  const root = makeRoot(context, { vendor: false })
  const sibling = mkdtempSync(join(tmpdir(), "nessa-sibling-"))
  context.after(() => rmSync(sibling, { recursive: true, force: true }))
  for (const directory of linkedWorkspacePackages) {
    mkdirSync(join(sibling, directory), { recursive: true })
    writeFileSync(join(sibling, directory, "package.json"), "{}")
  }
  mkdirSync(join(root, ".vendor"))
  symlinkSync(sibling, join(root, ".vendor/nessa_ui"))
  const calls = []
  assert.throws(
    () =>
      prepareVendor({
        root,
        git: fakeGit({ head: OTHER }),
        run: (command, args) => calls.push([command, ...args]),
      }),
    /links to a checkout at another commit/,
  )
  assert.deepEqual(calls, [])
})

test("prepare only builds when the clone is already at the pin", (context) => {
  const root = makeRoot(context)
  const calls = []
  prepareVendor({
    root,
    git: fakeGit({ head: PIN }),
    run: (command, args) => calls.push([command, ...args]),
  })
  assert.equal(calls[0][0], "pnpm")
  assert.equal(calls[0][1], "install")
})
