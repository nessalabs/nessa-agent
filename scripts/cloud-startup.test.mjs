// Real Bash/Python/tar, isolated HOME/PATH/tool fixtures; no network or node_modules.
import assert from "node:assert/strict"
import { execFileSync, spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

const scripts = dirname(fileURLToPath(import.meta.url))

function fixture(t, { installed = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), "nessa-cloud-startup-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const bin = join(root, "bin")
  const repository = join(root, "repository")
  const home = join(root, "home")
  for (const path of [bin, home, join(repository, "scripts/desktop")])
    mkdirSync(path, { recursive: true })
  const dispatcher = join(root, "tool.mjs")
  writeFileSync(
    dispatcher,
    `#!${process.execPath}\n` +
      String.raw`
import fs from 'node:fs'
import path from 'node:path'
import { spawnSync } from 'node:child_process'
const root = FIXTURE_ROOT
const args = process.argv.slice(2)
const tool = path.basename(process.argv[1])
const marker = name => path.join(root, name)
const exists = name => fs.existsSync(marker(name))
const touch = name => fs.writeFileSync(marker(name), '')
fs.appendFileSync(marker('calls'), JSON.stringify({tool,args})+'\n')
if (tool === 'timeout') {
  if (exists('stall-browser-repair')) process.exit(124)
  const result=spawnSync(args[2],args.slice(3),{env:process.env,stdio:'inherit'})
  process.exit(result.status ?? 1)
}
else if (tool === 'uname') console.log(args[0] === '-s' ? 'Linux' : 'x86_64')
else if (tool === 'dpkg-query') { if (!exists('packages') || (exists('missing-package') && fs.readFileSync(marker('missing-package'),'utf8')===args.at(-1))) process.exit(1); console.log('installed') }
else if (tool === 'sudo') { if (args.includes('install')) touch('packages') }
else if (tool === 'curl') {
  const output = args[args.indexOf('-o') + 1]
  const url = args.find(arg => arg.startsWith('https://'))
  fs.copyFileSync(marker(url.endsWith('SHASUMS256.txt') ? 'SHASUMS256.txt' : 'node.tar.xz'), output)
} else if (tool === 'npm') {
  const prefix = args[args.indexOf('--prefix') + 1]
  fs.symlinkSync(process.argv[1], path.join(prefix, 'bin/pnpm'))
} else if (tool === 'rustup') {
  if (args[0] === 'toolchain') touch('rust')
  else if (!exists('rust')) process.exit(1)
  else if (args[0] === 'component') console.log('rustfmt-x86_64-unknown-linux-gnu\nclippy-x86_64-unknown-linux-gnu')
  else console.log('rustc 1.99.1')
} else if (tool === 'rustc') console.log('rustc 1.99.1')
else if (tool === 'cargo') {
  if (args.includes('--offline') && !exists('cargo')) process.exit(1)
  touch('cargo')
} else if (tool === 'pnpm') {
  if (args.includes('--version')) console.log('11.9.0')
  else if (args[0] === 'install') {
    if (args.includes('--nested-ui')) {
      if (!exists('ui-dependency')) {
        if (process.env.PNPM_CONFIG_OFFLINE === 'true') process.exit(1)
        touch('ui-dependency'); touch('ui-network')
      }
      process.exit(0)
    }
    if (exists('preinstall-ui')) {
      const nested=spawnSync(process.execPath,[process.argv[1],'install','--frozen-lockfile','--nested-ui'],{env:process.env})
      if (nested.status !== 0) process.exit(nested.status ?? 1)
    }
    if (exists('fail-pnpm')) process.exit(1)
    if (args.includes('--offline') && !exists('pnpm')) process.exit(1)
    touch('pnpm')
    const mod = path.join(process.cwd(), 'node_modules/playwright')
    fs.mkdirSync(mod, {recursive:true})
    fs.writeFileSync(path.join(mod, 'package.json'), '{"type":"module","exports":"./index.js"}')
    fs.writeFileSync(path.join(mod, 'index.js'), 'import fs from \'node:fs\';const engine=file=>({executablePath:()=>file,launch:async()=>{if(!fs.existsSync('+JSON.stringify(marker('browser-libraries'))+'))throw new Error(\'missing browser runtime libraries\');return{newPage:async()=>({setContent:async()=>{},textContent:async()=>\'Nessa cloud startup\'}),close:async()=>{}}}});export const chromium=engine('+JSON.stringify(marker('chromium'))+');export const webkit=engine('+JSON.stringify(marker('webkit'))+');')
  } else if (args.includes('playwright')) {
    if(args.includes('--with-deps')) touch('browser-libraries')
    for (const name of ['chromium','webkit']) { touch(name); fs.chmodSync(marker(name),0o755) }
  }
} else if (tool === 'git') console.log('1111111111111111111111111111111111111111')
else if (tool === 'gh') console.log('gh version fixture')
else process.exit(2)
`.replace("FIXTURE_ROOT", JSON.stringify(root)),
  )
  chmodSync(dispatcher, 0o755)
  for (const name of [
    "timeout",
    "uname",
    "dpkg-query",
    "sudo",
    "curl",
    "npm",
    "rustup",
    "rustc",
    "cargo",
    "pnpm",
    "git",
    "gh",
  ])
    symlinkSync(dispatcher, join(bin, name))
  const node = join(root, "node")
  writeFileSync(
    node,
    `#!/bin/bash\nif [ "$1" = --version ]; then echo v24.22.0; else exec '${process.execPath.replaceAll("'", "'\\''")}' "$@"; fi\n`,
  )
  chmodSync(node, 0o755)
  symlinkSync(node, join(bin, "node"))
  copyFileSync(
    join(scripts, "desktop/install-linux-build-deps.sh"),
    join(repository, "scripts/desktop/install-linux-build-deps.sh"),
  )
  if (installed) writeFileSync(join(root, "packages"), "")
  writeFileSync(join(repository, "package.json"), '{"packageManager":"pnpm@11.9.0"}')
  writeFileSync(
    join(repository, "scripts/ensure-nessa-ui.mjs"),
    `import{existsSync,writeFileSync}from'node:fs';if(process.argv.includes('--check')){if(!existsSync('.ui-ready'))process.exit(1)}else writeFileSync('.ui-ready','')`,
  )
  const env = {
    ...process.env,
    TEST_FIXTURE: root,
    HOME: home,
    PATH: `${bin}:${process.env.PATH}`,
    NESSA_CLOUD_ROOT: join(root, "owned"),
  }
  const calls = () =>
    readFileSync(join(root, "calls"), "utf8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line))
  const run = (script, args = []) =>
    spawnSync("bash", [script, ...args], { cwd: repository, env, encoding: "utf8" })
  return { root, bin, repository, home, env, run, calls, node, dispatcher }
}

function bootstrapFixture(t) {
  const f = fixture(t, { installed: false })
  copyFileSync(
    join(scripts, "cloud-startup.sh"),
    join(f.repository, "scripts/cloud-startup.sh"),
  )
  const contents = join(f.root, "archive/node-v24.22.0-linux-x64/bin")
  mkdirSync(contents, { recursive: true })
  copyFileSync(f.node, join(contents, "node"))
  symlinkSync(f.dispatcher, join(contents, "npm"))
  execFileSync("tar", [
    "-cJf",
    join(f.root, "node.tar.xz"),
    "-C",
    join(f.root, "archive"),
    "node-v24.22.0-linux-x64",
  ])
  const checksum = createHash("sha256")
    .update(readFileSync(join(f.root, "node.tar.xz")))
    .digest("hex")
  writeFileSync(
    join(f.root, "SHASUMS256.txt"),
    `${checksum}  node-v24.22.0-linux-x64.tar.xz\n`,
  )
  const cargo = join(f.home, ".cargo/bin")
  mkdirSync(cargo, { recursive: true })
  for (const name of ["cargo", "rustup", "rustc"])
    symlinkSync(f.dispatcher, join(cargo, name))
  const prepare = (mode) =>
    f.run(
      mode === "setup"
        ? join(f.repository, "scripts/cloud-startup.sh")
        : join(f.env.NESSA_CLOUD_ROOT, "bootstrap.sh"),
      [mode, f.repository],
    )
  const receipt = () =>
    JSON.parse(readFileSync(join(f.env.NESSA_CLOUD_ROOT, "preparation.json"), "utf8"))
  return { ...f, prepare, receipt }
}

test("installed Linux build dependencies do not contact apt", (t) => {
  const f = fixture(t)
  const result = f.run(
    join(f.repository, "scripts/desktop/install-linux-build-deps.sh"),
    ["xvfb", "gh"],
  )
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(
    f.calls().filter((call) => call.tool === "sudo"),
    [],
    "already-installed packages must not contact apt",
  )
})

test("missing Linux dependencies use the canonical installation and extra packages", (t) => {
  const f = fixture(t, { installed: false })
  const result = f.run(
    join(f.repository, "scripts/desktop/install-linux-build-deps.sh"),
    ["xvfb", "gh"],
  )
  assert.equal(result.status, 0, result.stderr)
  const commands = f.calls().filter((call) => call.tool === "sudo")
  assert.equal(commands.length, 2)
  assert.ok(commands[0].args.includes("update"))
  assert.ok(commands[1].args.includes("libwebkit2gtk-4.1-dev"))
  assert.deepEqual(commands[1].args.slice(-2), ["xvfb", "gh"])
})

test("a missing extra package cannot hide behind installed core dependencies", (t) => {
  const f = fixture(t)
  writeFileSync(join(f.root, "missing-package"), "gh")
  const result = f.run(
    join(f.repository, "scripts/desktop/install-linux-build-deps.sh"),
    ["gh"],
  )
  assert.equal(result.status, 0, result.stderr)
  assert.equal(f.calls().filter((call) => call.tool === "sudo").length, 2)
})

test("cold preparation installs owned tools; warm maintenance verifies without network preparation", (t) => {
  const f = bootstrapFixture(t)
  const cold = f.prepare("setup")
  assert.equal(cold.status, 0, cold.stdout + cold.stderr)
  const first = f.receipt()
  assert.equal(first.mode, "setup")
  assert.equal(first.preexistingIdentity, false)
  assert.equal(first.previousPreparation, null)
  assert.deepEqual(first.steps, {
    node: "installed",
    pnpm: "installed",
    rust: "installed",
    ui: "prepared",
    dependencies: "fetched",
    cargo: "fetched",
    browsers: "installed",
  })
  const offset = f.calls().length
  const warm = f.prepare("maintenance")
  assert.equal(warm.status, 0, warm.stdout + warm.stderr)
  const second = f.receipt()
  assert.equal(second.mode, "maintenance")
  assert.equal(second.preexistingIdentity, true)
  assert.deepEqual(second.cacheIdentity, first.cacheIdentity)
  assert.equal(second.previousPreparation.completedAt, first.completedAt)
  assert.deepEqual(second.steps, {
    node: "reused",
    pnpm: "reused",
    rust: "reused",
    ui: "reused",
    dependencies: "offline",
    cargo: "offline",
    browsers: "reused",
  })
  const repeatedNetwork = f
    .calls()
    .slice(offset)
    .filter(
      ({ tool, args }) =>
        tool === "curl" ||
        tool === "sudo" ||
        tool === "npm" ||
        (tool === "rustup" && args[0] === "toolchain") ||
        (tool === "pnpm" &&
          (args.includes("playwright") ||
            (args[0] === "install" && !args.includes("--offline")))) ||
        (tool === "cargo" && args[0] === "fetch" && !args.includes("--offline")),
    )
  assert.deepEqual(
    repeatedNetwork,
    [],
    "warm capabilities must not repeat download/install/update work",
  )
  const shell = spawnSync(
    "env",
    [
      "-i",
      `HOME=${f.home}`,
      "PATH=/usr/local/bin:/usr/bin:/bin",
      "bash",
      "-lc",
      'node --version; pnpm --version; command -v pnpm; rustc --version; bash -c "node --version; pnpm --version; command -v pnpm; rustc --version"',
    ],
    { encoding: "utf8" },
  )
  assert.equal(shell.status, 0, shell.stderr)
  assert.equal(shell.stdout.split("v24.22.0").length - 1, 2)
  assert.equal(
    shell.stdout.split(join(f.env.NESSA_CLOUD_ROOT, "node/bin/pnpm")).length - 1,
    2,
  )
  const independent = bootstrapFixture(t)
  const independentSetup = independent.prepare("setup")
  assert.equal(independentSetup.status, 0, independentSetup.stderr)
  assert.notEqual(independent.receipt().cacheIdentity.id, first.cacheIdentity.id)
  const repeatedSetup = f.prepare("setup")
  assert.equal(repeatedSetup.status, 0, repeatedSetup.stderr)
  assert.equal(f.receipt().mode, "setup")
  assert.deepEqual(f.receipt().steps, second.steps)
  assert.deepEqual(f.receipt().cacheIdentity, first.cacheIdentity)
})

test("inherited pnpm cannot substitute for the owned binary", (t) => {
  const f = bootstrapFixture(t)
  const first = f.prepare("setup")
  assert.equal(first.status, 0, first.stderr)
  rmSync(join(f.env.NESSA_CLOUD_ROOT, "node/bin/pnpm"))
  const second = f.prepare("maintenance")
  assert.equal(second.status, 0, second.stderr)
  assert.equal(f.receipt().steps.pnpm, "installed")
})

test("checksum or preparation failure cannot publish a successful receipt", (t) => {
  const f = bootstrapFixture(t)
  writeFileSync(
    join(f.root, "SHASUMS256.txt"),
    `${"0".repeat(64)}  node-v24.22.0-linux-x64.tar.xz\n`,
  )
  const corrupt = f.prepare("setup")
  assert.notEqual(corrupt.status, 0)
  assert.match(corrupt.stderr, /checksum mismatch/)
  assert.throws(f.receipt, { code: "ENOENT" })
  const digest = createHash("sha256")
    .update(readFileSync(join(f.root, "node.tar.xz")))
    .digest("hex")
  writeFileSync(
    join(f.root, "SHASUMS256.txt"),
    `${digest}  node-v24.22.0-linux-x64.tar.xz\n`,
  )
  const prepared = f.prepare("setup")
  assert.equal(prepared.status, 0, prepared.stderr)
  const receiptPath = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
  const original = readFileSync(receiptPath, "utf8")
  writeFileSync(join(f.root, "fail-pnpm"), "")
  const failed = f.prepare("maintenance")
  assert.notEqual(failed.status, 0)
  assert.equal(readFileSync(receiptPath, "utf8"), original)
})

test("a missing browser is repaired without reinstalling the owned toolchain", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  rmSync(join(f.root, "webkit"))
  const offset = f.calls().length
  const repaired = f.prepare("maintenance")
  assert.equal(repaired.status, 0, repaired.stderr)
  assert.equal(f.receipt().steps.browsers, "installed")
  assert.equal(f.receipt().steps.node, "reused")
  assert.equal(f.receipt().steps.rust, "reused")
  assert.equal(
    f
      .calls()
      .slice(offset)
      .filter((call) => call.tool === "curl" || call.tool === "sudo").length,
    0,
  )
})

test("maintenance survives a task checkout that predates the provisioning source", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const first = f.receipt()
  rmSync(join(f.repository, "scripts/cloud-startup.sh"))
  rmSync(join(f.repository, "scripts/desktop/install-linux-build-deps.sh"))
  const maintained = f.prepare("maintenance")
  assert.equal(maintained.status, 0, maintained.stderr)
  assert.equal(f.receipt().prepareSha256, first.prepareSha256)
  assert.equal(f.receipt().nativeSha256, first.nativeSha256)
  assert.deepEqual(f.receipt().cacheIdentity, first.cacheIdentity)
})

test("a changed installed recipe cannot impersonate its successful setup", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const receiptPath = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
  const previous = readFileSync(receiptPath, "utf8")
  const native = join(f.env.NESSA_CLOUD_ROOT, "native-build-deps.sh")
  writeFileSync(native, readFileSync(native, "utf8") + "\n# altered installed artifact\n")
  const changed = f.prepare("maintenance")
  assert.notEqual(changed.status, 0)
  assert.match(changed.stderr, /does not match its setup receipt/)
  assert.equal(readFileSync(receiptPath, "utf8"), previous)
})

test("present browser executables do not hide missing runtime libraries", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  rmSync(join(f.root, "browser-libraries"))
  const offset = f.calls().length
  const repaired = f.prepare("maintenance")
  assert.equal(repaired.status, 0, repaired.stderr)
  assert.equal(f.receipt().steps.browsers, "installed")
  const calls = f
    .calls()
    .slice(offset)
    .filter(({ tool, args }) => tool === "pnpm" && args.includes("playwright"))
  assert.equal(calls.length, 1)
  assert.ok(calls[0].args.includes("--with-deps"))
})

test("a stalled browser repair is bounded and cannot publish success", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const previous = f.receipt()
  rmSync(join(f.root, "browser-libraries"))
  writeFileSync(join(f.root, "stall-browser-repair"), "")
  const result = f.prepare("maintenance")
  assert.equal(result.status, 124, result.stderr)
  assert.deepEqual(f.receipt(), previous)
  const calls = f.calls().filter(({ tool }) => tool === "timeout")
  assert.deepEqual(calls.at(-1).args, [
    "--kill-after=30s",
    "45m",
    "pnpm",
    "exec",
    "playwright",
    "install",
    "--with-deps",
    "chromium",
    "webkit",
  ])
})

test("nested UI dependency fetching cannot be reported as offline reuse", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  writeFileSync(join(f.root, "preinstall-ui"), "")
  const fetched = f.prepare("maintenance")
  assert.equal(fetched.status, 0, fetched.stderr)
  assert.equal(f.receipt().steps.dependencies, "fetched")
  assert.equal(readFileSync(join(f.root, "ui-network"), "utf8"), "")
  const reused = f.prepare("maintenance")
  assert.equal(reused.status, 0, reused.stderr)
  assert.equal(f.receipt().steps.dependencies, "offline")
})
