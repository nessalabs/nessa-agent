// Real Bash/Python/tar, isolated HOME/PATH/tool fixtures; no network or node_modules.
import assert from "node:assert/strict"
import { execFileSync, spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
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
  if (args.includes('--version') || args.includes('--dry-run')) {
    const runtime=spawnSync('node',['--version'],{encoding:'utf8'})
    if (runtime.status !== 0 || !runtime.stdout.startsWith('v24.')) process.exit(9)
    console.log('11.9.0'); process.exit(0)
  }
  const prefix = args[args.indexOf('--prefix') + 1]
  if (args[0] === 'uninstall') {
    fs.rmSync(path.join(prefix,'bin/pnpm'),{force:true}); touch('pnpm-removed')
  } else {
    fs.symlinkSync(fs.realpathSync(process.argv[1]), path.join(prefix, 'bin/pnpm'))
    if (exists('pnpm-removed') && !exists('fail-pnpm-repair')) fs.rmSync(marker('broken-pnpm-install'),{force:true})
    fs.rmSync(marker('pnpm-removed'),{force:true})
  }
} else if (tool === 'rustup') {
  if (args[0] === 'toolchain') {
    if (args[1] === 'uninstall') { fs.rmSync(marker('rust'),{force:true}); touch('rust-removed') }
    else {
      touch('rust')
      if (exists('rust-removed') && !exists('fail-rust-repair')) fs.rmSync(marker('broken-rust-command'),{force:true})
      fs.rmSync(marker('rust-removed'),{force:true})
    }
  }
  else if (!exists('rust')) process.exit(1)
  else if (args[0] === 'component') console.log('rustfmt-x86_64-unknown-linux-gnu\nclippy-x86_64-unknown-linux-gnu')
  else {
    if (args[0] === 'run' && exists('broken-rust-command') && args.slice(2,4).join(' ') === fs.readFileSync(marker('broken-rust-command'),'utf8')) process.exit(7)
    console.log('rustc 1.99.1')
  }
} else if (tool === 'rustc') console.log('rustc 1.99.1')
else if (tool === 'cargo') {
  if (args[0] === 'install') {
    if (exists('fail-just-install')) process.exit(101)
    const bin = path.join(args[args.indexOf('--root') + 1], 'bin')
    fs.mkdirSync(bin, {recursive:true})
    fs.writeFileSync(path.join(bin, 'just'), '#!/bin/sh\necho "just ' + args[args.indexOf('--version') + 1] + '"\n', {mode:0o755})
    process.exit(0)
  }
  if (args.includes('--offline') && !exists('cargo')) process.exit(1)
  touch('cargo')
} else if (tool === 'pnpm') {
  if (args.includes('--version')) console.log('11.9.0')
  else if (args[0] === 'install') {
    const manifest=JSON.parse(fs.readFileSync(path.join(process.cwd(),'package.json'),'utf8'))
    if (manifest.name === 'nessa-cloud-probe') {
      if (exists('containing-workspace') && !args.includes('--ignore-workspace')) { touch('parent-workspace-effect'); process.exit(7) }
      if (exists('broken-pnpm-install')) process.exit(fs.readFileSync(marker('broken-pnpm-install'),'utf8') === 'silent' ? 0 : 7)
      const pkg=path.join(process.cwd(),'node_modules/nessa-cloud-readiness'); fs.mkdirSync(pkg,{recursive:true})
      fs.writeFileSync(path.join(pkg,'package.json'),'{"main":"index.cjs"}');fs.writeFileSync(path.join(pkg,'index.cjs'),'module.exports = 42')
      process.exit(0)
    }
    if (exists('broken-pnpm-install')) process.exit(7)
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
    if (exists('invalidate-ui')) fs.rmSync(path.join(root,'repository','.ui-ready'), {force:true})
    const mod = path.join(process.cwd(), 'node_modules/playwright')
    fs.mkdirSync(mod, {recursive:true})
    fs.writeFileSync(path.join(mod, 'package.json'), '{"type":"module","exports":"./index.js"}')
    fs.writeFileSync(path.join(mod, 'index.js'), 'import fs from \'node:fs\';import path from \'node:path\';const engine=(file,linked)=>{const executablePath=()=>process.env.PLAYWRIGHT_BROWSERS_PATH?path.join(process.env.PLAYWRIGHT_BROWSERS_PATH,linked):file;return{executablePath,launch:async()=>{if(!fs.existsSync(executablePath()))throw new Error(\'missing browser\');if(!fs.existsSync('+JSON.stringify(marker('browser-libraries'))+'))throw new Error(\'missing browser runtime libraries\');return{newPage:async()=>({setContent:async()=>{},textContent:async()=>\'Nessa cloud startup\'}),close:async()=>{}}}}};export const chromium=engine('+JSON.stringify(marker('chromium'))+',\'chromium-1243/chrome-linux64/chrome\');export const webkit=engine('+JSON.stringify(marker('webkit'))+',\'webkit-2359/pw_run.sh\');')
  } else if (args.includes('playwright')) {
    if(args.includes('--with-deps')) touch('browser-libraries')
    if (exists('refuse-browser-download')) { console.log("Error: Download failed: server returned code 403 body 'request blocked'"); process.exit(1) }
    if (exists('fail-browser-install')) process.exit(1)
    const browsers = process.env.PLAYWRIGHT_BROWSERS_PATH
    if (browsers) for (const file of ['chromium-1243/chrome-linux64/chrome', 'webkit-2359/pw_run.sh']) { fs.mkdirSync(path.dirname(path.join(browsers, file)), {recursive:true}); fs.writeFileSync(path.join(browsers, file), '', {mode:0o755}) }
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
    `import{existsSync,writeFileSync,rmSync}from'node:fs';import{join}from'node:path';export const checkVendor=({root,requireOwned=false})=>requireOwned&&existsSync(join(root,'.ui-borrowed'))?'Borrowed UI fixture':existsSync(join(root,'.ui-ready'))?null:'UI fixture requires preparation';export const prepareVendor=({root})=>{writeFileSync(join(root,'.ui-ready'),'');rmSync(join(root,'.ui-borrowed'),{force:true})}`,
  )
  // A sandbox's own browsers would otherwise reach the fallback in every test.
  const { PLAYWRIGHT_BROWSERS_PATH: _sandboxBrowsers, ...inherited } = process.env
  const env = {
    ...inherited,
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
    just: "installed",
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
    just: "reused",
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
        (tool === "npm" &&
          args[0] === "install" &&
          !(args.includes("--dry-run") && args.includes("--offline"))) ||
        (tool === "rustup" && args[0] === "toolchain") ||
        (tool === "pnpm" &&
          (args.includes("playwright") ||
            (args[0] === "install" && !args.includes("--offline")))) ||
        (tool === "cargo" && args[0] === "install") ||
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
      'node --version; pnpm --version; command -v pnpm; rustc --version; just --version; bash -c "node --version; pnpm --version; command -v pnpm; rustc --version; just --version"',
    ],
    { encoding: "utf8" },
  )
  assert.equal(shell.status, 0, shell.stderr)
  assert.equal(shell.stdout.split("v24.22.0").length - 1, 2)
  assert.equal(shell.stdout.split("just 1.").length - 1, 2)
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
  assert.equal(
    spawnSync(join(f.env.NESSA_CLOUD_ROOT, "node/bin/pnpm"), ["--version"], {
      env: f.env,
    }).status,
    0,
  )
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
  rmSync(join(f.repository, "scripts/ensure-nessa-ui.mjs"))
  rmSync(join(f.repository, ".ui-ready"))
  const maintained = f.prepare("maintenance")
  assert.equal(maintained.status, 0, maintained.stderr)
  assert.equal(f.receipt().prepareSha256, first.prepareSha256)
  assert.equal(f.receipt().nativeSha256, first.nativeSha256)
  assert.equal(f.receipt().uiSha256, first.uiSha256)
  assert.equal(f.receipt().steps.ui, "prepared")
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

function sandboxBrowsers(f) {
  const sandbox = join(f.root, "sandbox-browsers")
  const chrome = join(sandbox, "chromium-1194/chrome-linux/chrome")
  const shell = join(sandbox, "chromium_headless_shell-1194/chrome-linux/headless_shell")
  for (const file of [chrome, shell]) {
    mkdirSync(dirname(file), { recursive: true })
    writeFileSync(file, "#!/bin/sh\n", { mode: 0o755 })
  }
  return { sandbox, chrome, shell }
}

test("a refused browser download falls back to the sandbox's own Chromium", (t) => {
  const f = bootstrapFixture(t)
  const { sandbox, chrome, shell } = sandboxBrowsers(f)
  writeFileSync(join(f.root, "refuse-browser-download"), "")
  f.env.PLAYWRIGHT_BROWSERS_PATH = sandbox
  const cold = f.prepare("setup")
  assert.equal(cold.status, 0, cold.stderr)
  assert.match(cold.stderr, /WebKit checks cannot run/)
  assert.equal(f.receipt().steps.browsers, "sandbox-chromium")
  const links = join(f.env.NESSA_CLOUD_ROOT, "browsers")
  assert.equal(realpathSync(join(links, "chromium-1243/chrome-linux64/chrome")), chrome)
  assert.equal(
    realpathSync(
      join(
        links,
        "chromium_headless_shell-1243/chrome-headless-shell-linux64/chrome-headless-shell",
      ),
    ),
    shell,
  )
  const loginShell = (environment) =>
    spawnSync(
      "env",
      [
        "-i",
        `HOME=${f.home}`,
        ...environment,
        "PATH=/usr/bin:/bin",
        "bash",
        "-lc",
        'printf "%s %s" "$PLAYWRIGHT_BROWSERS_PATH" "$NESSA_BROWSER_CHANNEL"',
      ],
      { encoding: "utf8" },
    ).stdout
  assert.equal(loginShell([]), `${links} bundled`)

  // A shell that sourced the helper hands the links back; the rerun still
  // starts from the sandbox's own browsers.
  f.env.PLAYWRIGHT_BROWSERS_PATH = links
  const warm = f.prepare("maintenance")
  assert.equal(warm.status, 0, warm.stderr)
  assert.equal(f.receipt().steps.browsers, "sandbox-chromium")
  assert.equal(realpathSync(join(links, "chromium-1243/chrome-linux64/chrome")), chrome)
  assert.equal(
    readFileSync(join(f.env.NESSA_CLOUD_ROOT, "browsers-source"), "utf8"),
    sandbox,
  )

  // A failed rerun keeps the links the earlier receipt describes.
  writeFileSync(join(f.root, "fail-just-install"), "")
  rmSync(join(f.env.NESSA_CLOUD_ROOT, "just"), { recursive: true })
  assert.notEqual(f.prepare("maintenance").status, 0)
  assert.equal(loginShell([]), `${links} bundled`)
  rmSync(join(f.root, "fail-just-install"))

  // Once the download works, Playwright's own browsers replace the links.
  rmSync(join(f.root, "refuse-browser-download"))
  const repaired = f.prepare("maintenance")
  assert.equal(repaired.status, 0, repaired.stderr)
  assert.equal(f.receipt().steps.browsers, "installed")
  assert.equal(existsSync(links), false)
  assert.equal(loginShell([`PLAYWRIGHT_BROWSERS_PATH=${sandbox}`]), `${sandbox} bundled`)
})

test("a browser install that fails for another reason does not fall back", (t) => {
  const f = bootstrapFixture(t)
  const { sandbox } = sandboxBrowsers(f)
  writeFileSync(join(f.root, "fail-browser-install"), "")
  f.env.PLAYWRIGHT_BROWSERS_PATH = sandbox
  const result = f.prepare("setup")
  assert.equal(result.status, 1, result.stderr)
  assert.equal(existsSync(join(f.env.NESSA_CLOUD_ROOT, "browsers")), false)
  assert.equal(existsSync(join(f.env.NESSA_CLOUD_ROOT, "preparation.json")), false)
})

test("a refused browser download with no sandbox browsers publishes nothing", (t) => {
  const f = bootstrapFixture(t)
  writeFileSync(join(f.root, "refuse-browser-download"), "")
  const result = f.prepare("setup")
  assert.equal(result.status, 1, result.stderr)
  assert.equal(existsSync(join(f.env.NESSA_CLOUD_ROOT, "preparation.json")), false)
})

test("a failed just install cannot publish success", (t) => {
  const f = bootstrapFixture(t)
  writeFileSync(join(f.root, "fail-just-install"), "")
  const result = f.prepare("setup")
  assert.equal(result.status, 101, result.stderr)
  assert.equal(existsSync(join(f.env.NESSA_CLOUD_ROOT, "preparation.json")), false)
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

test("invalid cloud command arguments report usage before preparation", () => {
  for (const args of [[], ["setup"], ["setup", ".", "extra"]]) {
    const result = spawnSync("bash", [join(scripts, "cloud-startup.sh"), ...args], {
      encoding: "utf8",
    })
    assert.equal(result.status, 2)
    assert.equal(result.stdout, "")
    assert.match(result.stderr, /Usage: cloud-startup.sh setup\|maintenance <checkout>/)
  }
})

test("changed installed UI preparation cannot replace verified ownership", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const receipt = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
  const previous = readFileSync(receipt, "utf8")
  const helper = join(f.env.NESSA_CLOUD_ROOT, "ui-workspace.mjs")
  assert.equal(
    createHash("sha256").update(readFileSync(helper)).digest("hex"),
    f.receipt().uiSha256,
  )
  writeFileSync(helper, "throw new Error('unverified UI owner ran')")
  const result = f.prepare("maintenance")
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /does not match its setup receipt/)
  assert.equal(readFileSync(receipt, "utf8"), previous)
  assert.doesNotMatch(result.stderr, /unverified UI owner ran/)
})

test("lost UI readiness after dependency work preserves the earlier receipt", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const receipt = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
  const previous = readFileSync(receipt, "utf8")
  writeFileSync(join(f.root, "invalidate-ui"), "")
  const result = f.prepare("maintenance")
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /UI fixture requires preparation/)
  assert.equal(readFileSync(receipt, "utf8"), previous)
})

for (const damage of ["missing", "broken"]) {
  test(`a ${damage} bundled npm repairs Node before replacing pnpm`, (t) => {
    const f = bootstrapFixture(t)
    assert.equal(f.prepare("setup").status, 0)
    const npm = join(f.env.NESSA_CLOUD_ROOT, "node/bin/npm")
    rmSync(npm)
    rmSync(join(f.env.NESSA_CLOUD_ROOT, "node/bin/pnpm"))
    if (damage === "broken") {
      writeFileSync(npm, "#!/bin/sh\nexit 7\n")
      chmodSync(npm, 0o755)
    }
    const repaired = f.prepare("maintenance")
    assert.equal(repaired.status, 0, repaired.stderr)
    assert.equal(f.receipt().steps.node, "installed")
    assert.equal(f.receipt().steps.pnpm, "installed")
    assert.equal(spawnSync(npm, ["--version"], { env: f.env }).status, 0)
  })
}

test("a failed bundled npm repair cannot publish new preparation", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const receipt = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
  const previous = readFileSync(receipt, "utf8")
  rmSync(join(f.env.NESSA_CLOUD_ROOT, "node/bin/npm"))
  const archiveNpm = join(f.root, "archive/node-v24.22.0-linux-x64/bin/npm")
  rmSync(archiveNpm)
  writeFileSync(archiveNpm, "#!/bin/sh\nexit 7\n")
  chmodSync(archiveNpm, 0o755)
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
  const failed = f.prepare("maintenance")
  assert.notEqual(failed.status, 0)
  assert.equal(readFileSync(receipt, "utf8"), previous)
})

test("bundled npm readiness uses owned Node before publishing the shell helper", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const inherited = join(f.bin, "node")
  rmSync(inherited)
  writeFileSync(inherited, "#!/bin/sh\necho v18.0.0\n")
  chmodSync(inherited, 0o755)
  const maintained = f.prepare("maintenance")
  assert.equal(maintained.status, 0, maintained.stderr)
  assert.equal(f.receipt().steps.node, "reused")
})

for (const mode of ["setup", "maintenance"]) {
  for (const missing of ["mode", "completedAt", "sourceHead", "prepareSha256"]) {
    test(`${mode} reconciles capabilities with a missing prior ${missing}`, (t) => {
      const f = bootstrapFixture(t)
      assert.equal(f.prepare("setup").status, 0)
      const prior = f.receipt()
      delete prior[missing]
      writeFileSync(
        join(f.env.NESSA_CLOUD_ROOT, "preparation.json"),
        JSON.stringify(prior),
      )
      const reconciled = f.prepare(mode)
      assert.equal(reconciled.status, 0, reconciled.stderr)
      const current = f.receipt()
      assert.deepEqual(current.cacheIdentity, prior.cacheIdentity)
      assert.equal(current.preexistingIdentity, true)
      assert.equal(current.mode, mode)
      assert.deepEqual(
        current.previousPreparation,
        Object.fromEntries(
          ["mode", "completedAt", "sourceHead", "prepareSha256"].map((key) => [
            key,
            key === missing ? null : prior[key],
          ]),
        ),
      )
      assert.equal(current.steps.node, "reused")
      assert.equal(current.steps.dependencies, "offline")
    })
  }
}

test("failed repair preserves an incomplete prior receipt", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const receipt = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
  const prior = f.receipt()
  delete prior.completedAt
  const previous = JSON.stringify(prior)
  writeFileSync(receipt, previous)
  writeFileSync(join(f.root, "fail-pnpm"), "")
  const offset = f.calls().length
  const failed = f.prepare("maintenance")
  assert.notEqual(failed.status, 0)
  assert.equal(readFileSync(receipt, "utf8"), previous)
  assert.ok(
    f
      .calls()
      .slice(offset)
      .some(({ tool, args }) => tool === "pnpm" && args.includes("--offline")),
  )
})

test("npm version alone cannot hide a missing install dependency", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const npm = join(f.env.NESSA_CLOUD_ROOT, "node/bin/npm")
  rmSync(npm)
  writeFileSync(
    npm,
    '#!/bin/sh\nif [ "$1" = "--version" ]; then echo 11.19.1; exit 0; fi\nif [ "$1" = "uninstall" ]; then exit 0; fi\necho "Cannot find module npm/lib/commands/install.js" >&2\nexit 7\n',
  )
  chmodSync(npm, 0o755)
  rmSync(join(f.env.NESSA_CLOUD_ROOT, "node/bin/pnpm"))
  assert.equal(spawnSync(npm, ["--version"], { env: f.env }).status, 0)
  const repaired = f.prepare("maintenance")
  assert.equal(repaired.status, 0, repaired.stderr)
  assert.equal(f.receipt().steps.node, "installed")
  assert.equal(f.receipt().steps.pnpm, "installed")
})

test("failed npm install capability after extraction preserves prior preparation", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const receipt = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
  const prior = readFileSync(receipt, "utf8")
  rmSync(join(f.env.NESSA_CLOUD_ROOT, "node/bin/npm"))
  const archiveNpm = join(f.root, "archive/node-v24.22.0-linux-x64/bin/npm")
  rmSync(archiveNpm)
  writeFileSync(
    archiveNpm,
    '#!/bin/sh\nif [ "$1" = "--version" ]; then echo 11.19.1; exit 0; fi\nexit 7\n',
  )
  chmodSync(archiveNpm, 0o755)
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
  const failed = f.prepare("maintenance")
  assert.notEqual(failed.status, 0)
  assert.equal(readFileSync(receipt, "utf8"), prior)
})

for (const command of ["cargo check", "cargo fmt", "cargo clippy"]) {
  test(`stable ${command} capability is exercised before reuse`, (t) => {
    const f = bootstrapFixture(t)
    assert.equal(f.prepare("setup").status, 0)
    writeFileSync(join(f.root, "broken-rust-command"), command)
    const repaired = f.prepare("maintenance")
    assert.equal(repaired.status, 0, repaired.stderr)
    assert.equal(f.receipt().steps.rust, "installed")
    assert.ok(
      f
        .calls()
        .some(
          ({ tool, args }) =>
            tool === "rustup" && args[0] === "toolchain" && args[1] === "uninstall",
        ),
    )
  })
}

test("failed stable capability repair preserves prior preparation", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const receipt = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
  const previous = readFileSync(receipt, "utf8")
  writeFileSync(join(f.root, "broken-rust-command"), "cargo fmt")
  writeFileSync(join(f.root, "fail-rust-repair"), "")
  const failed = f.prepare("maintenance")
  assert.notEqual(failed.status, 0)
  assert.equal(readFileSync(receipt, "utf8"), previous)
})

for (const failure of ["exit", "silent"]) {
  test(`pnpm import capability repairs a ${failure} missing-worker failure`, (t) => {
    const f = bootstrapFixture(t)
    assert.equal(f.prepare("setup").status, 0)
    writeFileSync(join(f.root, "broken-pnpm-install"), failure)
    const repaired = f.prepare("maintenance")
    assert.equal(repaired.status, 0, repaired.stderr)
    assert.equal(f.receipt().steps.pnpm, "installed")
  })
}

test("failed pnpm capability repair stops before checkout dependencies and preserves prior preparation", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const receipt = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
  const previous = readFileSync(receipt, "utf8")
  writeFileSync(join(f.root, "broken-pnpm-install"), "silent")
  writeFileSync(join(f.root, "fail-pnpm-repair"), "")
  const offset = f.calls().length
  const failed = f.prepare("maintenance")
  assert.notEqual(failed.status, 0)
  assert.equal(readFileSync(receipt, "utf8"), previous)
  assert.equal(
    f
      .calls()
      .slice(offset)
      .filter(
        ({ tool, args }) =>
          tool === "pnpm" &&
          args[0] === "install" &&
          !args.includes("--ignore-workspace"),
      ).length,
    0,
  )
})

test("npm uninstall capability is verified before reusing the bundle", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  const npm = join(f.env.NESSA_CLOUD_ROOT, "node/bin/npm")
  rmSync(npm)
  writeFileSync(
    npm,
    '#!/bin/sh\nif [ "$1" = "--version" ]; then echo 11.19.1; exit 0; fi\nif [ "$1" = "install" ]; then exit 0; fi\nexit 7\n',
  )
  chmodSync(npm, 0o755)
  const repaired = f.prepare("maintenance")
  assert.equal(repaired.status, 0, repaired.stderr)
  assert.equal(f.receipt().steps.node, "installed")
})

test("pnpm capability probe ignores a containing workspace", (t) => {
  const f = bootstrapFixture(t)
  writeFileSync(join(f.root, "containing-workspace"), "")
  const prepared = f.prepare("setup")
  assert.equal(prepared.status, 0, prepared.stderr)
  assert.throws(() => readFileSync(join(f.root, "parent-workspace-effect")), {
    code: "ENOENT",
  })
})

for (const [owner, allocation] of [
  ["npm", 1],
  ["pnpm", 2],
  ["Rust", 3],
]) {
  for (const failure of ["allocation", "fixture-write"]) {
    test(`${owner} probe ${failure} failure preserves cached tools and prior preparation`, (t) => {
      const f = bootstrapFixture(t)
      assert.equal(f.prepare("setup").status, 0)
      const receipt = join(f.env.NESSA_CLOUD_ROOT, "preparation.json")
      const previous = readFileSync(receipt, "utf8")
      const realMktemp = execFileSync("sh", ["-c", "command -v mktemp"], {
        encoding: "utf8",
      }).trim()
      const wrapper = join(f.bin, "mktemp")
      writeFileSync(
        wrapper,
        `#!${process.execPath}\n` +
          `
import fs from 'node:fs'
import { spawnSync } from 'node:child_process'
const args = process.argv.slice(2)
const counter = ${JSON.stringify(join(f.root, "probe-count"))}
let selected = false
if (args.includes('-d')) {
  const count = fs.existsSync(counter) ? Number(fs.readFileSync(counter, 'utf8')) + 1 : 1
  fs.writeFileSync(counter, String(count))
  selected = count === ${allocation}
}
if (selected && ${JSON.stringify(failure)} === 'allocation') {
  console.error('private probe allocation unavailable'); process.exit(1)
}
const result = spawnSync(${JSON.stringify(realMktemp)}, args, {encoding: 'utf8'})
if (selected && result.status === 0) {
  const root = result.stdout.trim()
  if (${allocation} === 1) fs.mkdirSync(root + '/package.json')
  else if (${allocation} === 2) fs.mkdirSync(root + '/dependency.tgz')
  else fs.writeFileSync(root + '/src', 'blocking file')
}
process.stdout.write(result.stdout ?? '')
process.stderr.write(result.stderr ?? '')
process.exit(result.status ?? 1)
`,
      )
      chmodSync(wrapper, 0o755)
      const offset = f.calls().length
      const failed = f.prepare("maintenance")
      assert.equal(failed.status, 2, failed.stderr)
      assert.equal(readFileSync(receipt, "utf8"), previous)
      assert.deepEqual(
        f
          .calls()
          .slice(offset)
          .filter(
            ({ tool, args }) =>
              tool === "curl" ||
              (tool === "npm" && !args.includes("--dry-run")) ||
              (tool === "rustup" && args[0] === "toolchain"),
          ),
        [],
        "probe environment failure must not replace a cached tool",
      )
    })
  }
}

test("cloud readiness requires private UI ownership despite ready outputs", (t) => {
  const f = bootstrapFixture(t)
  assert.equal(f.prepare("setup").status, 0)
  writeFileSync(join(f.repository, ".ui-borrowed"), "borrowed ready outputs")
  const repaired = f.prepare("maintenance")
  assert.equal(repaired.status, 0, repaired.stderr)
  assert.equal(f.receipt().steps.ui, "prepared")
  assert.throws(() => readFileSync(join(f.repository, ".ui-borrowed")), {
    code: "ENOENT",
  })
})
