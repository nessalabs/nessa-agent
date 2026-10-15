import assert from "node:assert/strict"
import {
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { spawnSync } from "node:child_process"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import { pathToFileURL } from "node:url"
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

test("workspace preparation detaches borrowed node_modules roots without changing their targets", (context) => {
  const parent = mkdtempSync(join(tmpdir(), "nessa-borrowed-dependencies-"))
  context.after(() => rmSync(parent, { recursive: true, force: true }))
  const root = join(parent, "workspace")
  const borrowed = join(parent, "borrowed")
  mkdirSync(root)
  mkdirSync(borrowed)
  const roots = [
    "node_modules",
    ...linkedWorkspacePackages.map((pkg) => `${pkg}/node_modules`),
  ]
  for (const [index, relativeRoot] of roots.entries()) {
    const target = join(borrowed, String(index))
    mkdirSync(target, { recursive: true })
    writeFileSync(join(target, "marker"), `borrowed ${index}`)
    const nodeModules = join(root, relativeRoot)
    mkdirSync(join(nodeModules, ".."), { recursive: true })
    symlinkSync(target, nodeModules, "junction")
  }
  for (const directory of linkedWorkspacePackages) {
    mkdirSync(join(root, directory), { recursive: true })
    writeFileSync(
      join(root, directory, "package.json"),
      JSON.stringify({ name: directory, scripts: { build: "fake-build" } }),
    )
  }

  const calls = []
  ensureWorkspace(root, (command, args, cwd) => {
    calls.push({ command, cwd })
    assert.equal(cwd, root, "install and build run in the owned workspace")
    for (const relativeRoot of roots)
      assert.equal(
        lstatSync(join(root, relativeRoot), { throwIfNoEntry: false })?.isSymbolicLink(),
        undefined,
        `${relativeRoot} is detached before ${command} ${args.join(" ")}`,
      )
  })

  assert.deepEqual(
    calls.map(({ command }) => command),
    ["pnpm", "pnpm", "pnpm"],
  )
  for (const [index] of roots.entries())
    assert.equal(
      readFileSync(join(borrowed, String(index), "marker"), "utf8"),
      `borrowed ${index}`,
    )
})

test("workspace preparation preserves package links inside owned node_modules directories", (context) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-owned-dependencies-"))
  context.after(() => rmSync(root, { recursive: true, force: true }))
  for (const directory of linkedWorkspacePackages) {
    mkdirSync(join(root, directory), { recursive: true })
    writeFileSync(
      join(root, directory, "package.json"),
      JSON.stringify({ name: directory }),
    )
  }
  const roots = [
    "node_modules",
    ...linkedWorkspacePackages.map((pkg) => `${pkg}/node_modules`),
  ]
  for (const [index, relativeRoot] of roots.entries()) {
    const packageTarget = join(root, relativeRoot, ".pnpm", `package-${index}`)
    mkdirSync(packageTarget, { recursive: true })
    writeFileSync(join(packageTarget, "owned"), "local package")
    symlinkSync(
      join(root, relativeRoot, ".pnpm", `package-${index}`),
      join(root, relativeRoot, `package-${index}`),
      "junction",
    )
  }

  ensureWorkspace(root, () => {})

  for (const [index, relativeRoot] of roots.entries()) {
    assert.equal(lstatSync(join(root, relativeRoot)).isSymbolicLink(), false)
    assert.equal(
      lstatSync(join(root, relativeRoot, `package-${index}`)).isSymbolicLink(),
      true,
      `${relativeRoot} keeps its package link`,
    )
    assert.equal(
      readFileSync(join(root, relativeRoot, `package-${index}`, "owned"), "utf8"),
      "local package",
    )
  }
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

/** Simulate Git checkout and builders while recording their exact owner. */
function privateBuilder(vendor, calls) {
  let head = OTHER
  return {
    git(args, cwd) {
      calls.push({ command: "query", args, cwd })
      assert.equal(cwd, vendor)
      if (args[0] === "rev-parse") return head
      if (args[0] === "status") return ""
      return null
    },
    run(command, args, cwd) {
      calls.push({ command, args, cwd })
      if (command === "git" && args[0] === "clone") {
        assert.equal(args.at(-1), vendor)
        assert.ok(!existsSync(vendor), "clone starts without a borrowed link")
        for (const directory of linkedWorkspacePackages) {
          mkdirSync(join(vendor, directory), { recursive: true })
          writeFileSync(
            join(vendor, directory, "package.json"),
            JSON.stringify({ name: directory, scripts: { build: "fake-build" } }),
          )
        }
      } else {
        assert.equal(cwd, vendor)
        assert.ok(!lstatSync(vendor).isSymbolicLink())
        if (command === "git" && args[0] === "checkout") head = args.at(-1)
        if (command === "pnpm") writeFileSync(join(cwd, "build-artifact"), args.join(" "))
      }
    },
  }
}

function borrowedWorkspace(context, root) {
  const sibling = join(root, "../nessa_ui")
  context.after(() => rmSync(sibling, { recursive: true, force: true }))
  for (const directory of linkedWorkspacePackages) {
    mkdirSync(join(sibling, directory), { recursive: true })
    writeFileSync(join(sibling, directory, "package.json"), "{}")
  }
  writeFileSync(join(sibling, "dirty-source"), "local edits")
  writeFileSync(join(sibling, "build-artifact"), "frozen browser input")
  return sibling
}

test("prepare clones privately even when a matching sibling exists", (context) => {
  // Put the calling checkout and its candidate sibling under one private fixture.
  const parent = mkdtempSync(join(tmpdir(), "nessa-siblings-"))
  context.after(() => rmSync(parent, { recursive: true, force: true }))
  const root = join(parent, "agent")
  mkdirSync(root)
  writeFileSync(join(root, "nessa-ui-revision"), PIN)
  const sibling = borrowedWorkspace(context, root)
  const vendor = join(root, ".vendor/nessa_ui")
  const calls = []
  const builder = privateBuilder(vendor, calls)
  prepareVendor({
    root,
    run: builder.run,
    git: (args, cwd) => (cwd === sibling ? PIN : builder.git(args, cwd)),
  })
  assert.equal(calls[0].args[0], "clone")
  assert.ok(calls.some(({ command }) => command === "pnpm"))
  assert.equal(readFileSync(join(sibling, "dirty-source"), "utf8"), "local edits")
  assert.equal(
    readFileSync(join(sibling, "build-artifact"), "utf8"),
    "frozen browser input",
  )
})

test("matching siblings do not replace an owned clone with local edits", (context) => {
  const parent = mkdtempSync(join(tmpdir(), "nessa-owned-edits-"))
  context.after(() => rmSync(parent, { recursive: true, force: true }))
  const root = join(parent, "agent")
  mkdirSync(root)
  writeFileSync(join(root, "nessa-ui-revision"), PIN)
  const sibling = borrowedWorkspace(context, root)
  const vendor = join(root, ".vendor/nessa_ui")
  for (const directory of linkedWorkspacePackages) {
    mkdirSync(join(vendor, directory), { recursive: true })
    writeFileSync(join(vendor, directory, "package.json"), "{}")
  }
  writeFileSync(join(vendor, "dirty-source"), "owned local edits")
  const calls = []
  assert.throws(
    () =>
      prepareVendor({
        root,
        git: (args, cwd) =>
          cwd === sibling
            ? PIN
            : fakeGit({ head: OTHER, status: " M dirty-source" })(args),
        run: (...args) => calls.push(args),
      }),
    /Preserving changes/,
  )
  assert.deepEqual(calls, [])
  assert.equal(lstatSync(vendor).isSymbolicLink(), false)
  assert.equal(readFileSync(join(vendor, "dirty-source"), "utf8"), "owned local edits")
})

for (const head of [PIN, OTHER]) {
  for (const dirty of ["", " M dirty-source"]) {
    test(`prepare replaces a borrowed link at ${head.slice(0, 7)} with ${dirty ? "dirty" : "clean"} source`, (context) => {
      const parent = mkdtempSync(join(tmpdir(), "nessa-borrowed-"))
      context.after(() => rmSync(parent, { recursive: true, force: true }))
      const root = join(parent, "agent")
      mkdirSync(root)
      writeFileSync(join(root, "nessa-ui-revision"), PIN)
      const sibling = borrowedWorkspace(context, root)
      const vendor = join(root, ".vendor/nessa_ui")
      mkdirSync(join(root, ".vendor"))
      symlinkSync(sibling, vendor, "junction")
      const calls = []
      const builder = privateBuilder(vendor, calls)
      prepareVendor({
        root,
        run: builder.run,
        git: (args, cwd) => {
          assert.notEqual(resolve(cwd), resolve(sibling))
          // If preparation follows the vendor link, it sees the borrowed state.
          if (lstatSync(vendor).isSymbolicLink())
            return args[0] === "status" ? dirty : head
          return builder.git(args, cwd)
        },
      })
      assert.equal(calls[0].args[0], "clone")
      assert.equal(lstatSync(vendor).isSymbolicLink(), false)
      assert.ok(calls.some(({ command }) => command === "pnpm"))
      assert.equal(readFileSync(join(sibling, "dirty-source"), "utf8"), "local edits")
      assert.equal(
        readFileSync(join(sibling, "build-artifact"), "utf8"),
        "frozen browser input",
      )
    })
  }
}

test("prepare replaces a dangling vendor link with its own clone", (context) => {
  const root = makeRoot(context, { vendor: false })
  const vendor = join(root, ".vendor/nessa_ui")
  const absent = join(root, "absent")
  mkdirSync(join(root, ".vendor"))
  symlinkSync(absent, vendor, "junction")
  const calls = []
  prepareVendor({ root, ...privateBuilder(vendor, calls) })
  assert.equal(lstatSync(vendor).isSymbolicLink(), false)
  assert.equal(existsSync(absent), false)
  assert.ok(calls.some(({ command }) => command === "pnpm"))
})

test("a linked vendor parent refuses before touching its target", (context) => {
  const root = makeRoot(context, { vendor: false })
  const target = join(root, "borrowed-vendor")
  mkdirSync(target)
  writeFileSync(join(target, "artifact"), "frozen")
  symlinkSync(target, join(root, ".vendor"), "junction")
  const calls = []
  const rejectCall = (...args) => calls.push(args)
  assert.throws(
    () => prepareVendor({ root, run: rejectCall, git: rejectCall }),
    /Preserving linked .vendor directory/,
  )
  assert.deepEqual(calls, [])
  assert.equal(lstatSync(join(root, ".vendor")).isSymbolicLink(), true)
  assert.equal(readFileSync(join(target, "artifact"), "utf8"), "frozen")
  assert.equal(existsSync(join(target, "nessa_ui")), false)
})

test("failed private cloning preserves the borrowed target and allows retry", (context) => {
  const root = makeRoot(context, { vendor: false })
  const sibling = join(root, "borrowed-ui")
  mkdirSync(sibling)
  writeFileSync(join(sibling, "artifact"), "frozen")
  mkdirSync(join(root, ".vendor"))
  const vendor = join(root, ".vendor/nessa_ui")
  symlinkSync(sibling, vendor, "junction")
  const calls = []
  assert.throws(
    () =>
      prepareVendor({
        root,
        git: () => assert.fail("Git query before clone"),
        run: (command, args) => {
          calls.push([command, ...args])
          throw new Error("clone refused")
        },
      }),
    /clone refused/,
  )
  assert.equal(calls.length, 1)
  assert.equal(calls[0][1], "clone")
  assert.equal(existsSync(vendor), false)
  assert.equal(readFileSync(join(sibling, "artifact"), "utf8"), "frozen")
  prepareVendor({ root, ...privateBuilder(vendor, []) })
  assert.equal(lstatSync(vendor).isSymbolicLink(), false)
})

test("offline check inspects a borrowed link without preparing it", (context) => {
  const root = makeRoot(context, { vendor: false })
  const sibling = join(root, "borrowed-ui")
  for (const directory of linkedWorkspacePackages) {
    mkdirSync(join(sibling, directory), { recursive: true })
    writeFileSync(join(sibling, directory, "package.json"), "{}")
  }
  mkdirSync(join(root, ".vendor"))
  const vendor = join(root, ".vendor/nessa_ui")
  symlinkSync(sibling, vendor, "junction")
  const calls = []
  assert.equal(
    checkVendor({
      root,
      git: (args, cwd) => {
        calls.push({ args, cwd })
        return PIN
      },
    }),
    null,
  )
  assert.deepEqual(calls, [{ args: ["rev-parse", "HEAD"], cwd: vendor }])
  assert.equal(lstatSync(vendor).isSymbolicLink(), true)
})

test("private preparation runs in bare Node with a fake builder", (context) => {
  const root = makeRoot(context, { vendor: false })
  const module = join(root, "ensure-nessa-ui.mjs")
  copyFileSync(new URL("./ensure-nessa-ui.mjs", import.meta.url), module)
  const script = `
    import { prepareVendor } from ${JSON.stringify(pathToFileURL(module).href)};
    const root = ${JSON.stringify(root)};
    const calls = [];
    try { prepareVendor({root, git: () => null, run: (command, args) => {
      calls.push([command, ...args]);
      throw new Error("fake clone refusal");
    }}); } catch (error) {
      if (error.message !== "fake clone refusal") throw error;
    }
    if (calls.length !== 1 || calls[0][1] !== "clone") process.exit(1);
    console.log("bare Node clone path reached");
  `
  const result = spawnSync(process.execPath, ["--input-type=module", "--eval", script], {
    cwd: root,
    encoding: "utf8",
    timeout: 10000,
  })
  assert.ifError(result.error)
  assert.equal(result.status, 0, result.stderr)
  assert.equal(result.stderr, "")
  assert.equal(result.stdout.trim(), "bare Node clone path reached")
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

for (const damage of ["missing", "empty", "directory"]) {
  test(`check refuses ${damage} declared workspace outputs at the correct pin`, (context) => {
    const root = makeRoot(context)
    for (const directory of linkedWorkspacePackages) {
      const pkg = join(root, ".vendor/nessa_ui", directory)
      const exports = {
        ".": { types: "./dist/index.d.ts", import: "./dist/index.js" },
        "./styles.css": "./dist/styles.css",
      }
      writeFileSync(
        join(pkg, "package.json"),
        JSON.stringify({
          name: directory,
          main: "./dist/index.js",
          module: "./dist/index.js",
          types: "./dist/index.d.ts",
          exports,
        }),
      )
      mkdirSync(join(pkg, "dist"), { recursive: true })
      for (const output of ["index.js", "index.d.ts", "styles.css"])
        writeFileSync(join(pkg, "dist", output), "built output")
    }
    assert.equal(checkVendor({ root, git: fakeGit({ head: PIN }) }), null)
    for (const directory of linkedWorkspacePackages) {
      for (const output of ["index.js", "index.d.ts", "styles.css"]) {
        const file = join(root, ".vendor/nessa_ui", directory, "dist", output)
        rmSync(file)
        if (damage === "empty") writeFileSync(file, "")
        if (damage === "directory") mkdirSync(file)
        const problem = checkVendor({ root, git: fakeGit({ head: PIN }) })
        assert.match(problem, /pnpm ui:types/)
        assert.match(problem, new RegExp(output.replaceAll(".", "\\.")))
        rmSync(file, { recursive: true, force: true })
        writeFileSync(file, "rebuilt output")
        assert.equal(checkVendor({ root, git: fakeGit({ head: PIN }) }), null)
      }
    }
  })
}

for (const field of ["main", "module", "types", "typesVersions"]) {
  test(`check follows the package-owned ${field} entry`, (context) => {
    const root = makeRoot(context)
    const pkg = join(root, ".vendor/nessa_ui", linkedWorkspacePackages[0])
    const target = "./entry.js"
    writeFileSync(
      join(pkg, "package.json"),
      JSON.stringify({
        name: "fixture",
        [field]: field === "typesVersions" ? { "*": { entry: [target] } } : target,
      }),
    )
    writeFileSync(join(pkg, target), "built output")
    assert.equal(checkVendor({ root, git: fakeGit({ head: PIN }) }), null)
    rmSync(join(pkg, target))
    assert.match(
      checkVendor({ root, git: fakeGit({ head: PIN }) }),
      /entry.js.*pnpm ui:types/,
    )
  })
}
