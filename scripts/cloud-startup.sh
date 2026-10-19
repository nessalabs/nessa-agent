#!/usr/bin/env bash
# Setup and maintenance use this same capability reconciliation. Receipts describe
# observed reuse; they do not claim control over the platform's cache scheduler.
# Cold/warm/failure and inherited-tool regressions: cloud-startup.test.mjs.
set -euo pipefail
umask 077
[ "$#" -eq 2 ] || { echo 'Usage: cloud-startup.sh setup|maintenance <checkout>' >&2; exit 2; }
mode=$1
node_major=24
# CI installs the latest just (taiki-e/install-action); cloud pins one release.
just_version=1.59.0
case "$mode" in setup|maintenance) ;; *) echo 'Expected setup or maintenance' >&2; exit 2 ;; esac
[ "$(uname -s)" = Linux ] || { echo 'Cloud startup requires Linux.' >&2; exit 2; }
script_file=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")
repository=$(cd "${2:?Expected the task checkout directory}" && pwd)
cd "$repository"
cloud_root=${NESSA_CLOUD_ROOT:-/opt/nessa-cloud}
case "$cloud_root" in /*) ;; *) echo 'NESSA_CLOUD_ROOT must be absolute.' >&2; exit 2 ;; esac
state=$(mktemp)
node_download=''
trap 'rm -f "$state"; if [ -n "$node_download" ]; then rm -rf "$node_download"; fi' EXIT

# Capture evidence before creating or changing owned tool state. Identity is
# minted inside the container, not embedded as a constant in environment settings.
python3 - "$cloud_root" "$state" <<'PY'
import datetime, json, pathlib, sys, uuid
root, state = map(pathlib.Path, sys.argv[1:])
identity = root / 'cache-identity.json'
previous = json.loads(identity.read_text()) if identity.exists() else None
if previous is not None:
    uuid.UUID(previous['id'])
receipt = root / 'preparation.json'
prior = json.loads(receipt.read_text()) if receipt.exists() else None
state.write_text(json.dumps({
    'cacheIdentity': previous or {'id': str(uuid.uuid4()), 'createdAt': datetime.datetime.now(datetime.timezone.utc).isoformat()},
    'preexistingIdentity': previous is not None,
    'previousPreparation': {key: prior.get(key) for key in ('mode', 'completedAt', 'sourceHead', 'prepareSha256')} if prior else None,
}))
PY

mkdir -p "$cloud_root/node"
# Setup installs the provisioning recipe into the cached environment. A resumed
# task's branch can predate this source. Installed files remain generated artifacts.
python3 - "$cloud_root" "$repository" "$script_file" "$mode" <<'PY'
import hashlib, json, pathlib, subprocess, sys
root, repository, script = map(pathlib.Path, sys.argv[1:4])
deployment = root / 'bootstrap.json'
sources = {'bootstrap.sh': script, 'native-build-deps.sh': repository / 'scripts/desktop/install-linux-build-deps.sh', 'ui-workspace.mjs': repository / 'scripts/ensure-nessa-ui.mjs'}
if sys.argv[4] == 'setup':
    hashes = {}
    for name, source in sources.items():
        content = source.read_bytes()
        temporary = root / (name + '.tmp')
        temporary.write_bytes(content)
        temporary.replace(root / name)
        hashes[name] = hashlib.sha256(content).hexdigest()
    record = {'sourceHead': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), 'hashes': hashes}
    temporary = root / 'bootstrap.json.tmp'
    temporary.write_text(json.dumps(record))
    temporary.replace(deployment)
else:
    record = json.loads(deployment.read_text())
    for name in sources:
        if hashlib.sha256((root / name).read_bytes()).hexdigest() != record['hashes'][name]:
            raise SystemExit('Cached provisioning artifact does not match its setup receipt.')
PY
# Version output alone does not load npm's install-only dependencies. Probe the
# capability pnpm repair needs, offline and without scripts or dependency writes.
# Probe fixture creation exits this shell on error; only capability failures return
# to a repair caller. Allocation/write regressions cover all three probe owners.
node_ready() {
  [ -x "$cloud_root/node/bin/node" ] &&
    [[ "$("$cloud_root/node/bin/node" --version)" == "v${node_major}."* ]] || return 1
  local PATH="$cloud_root/node/bin:$PATH"
  local probe
  probe=$(mktemp -d) || exit 2
  printf '{"private":true}\n' > "$probe/package.json" || { rm -rf "$probe"; exit 2; }
  local args=(--dry-run --offline --ignore-scripts --package-lock=false --global=false --prefix "$probe" --cache "$probe/cache")
  local result=0
  "$cloud_root/node/bin/npm" install "${args[@]}" >/dev/null 2>&1 &&
    "$cloud_root/node/bin/npm" uninstall pnpm "${args[@]}" >/dev/null 2>&1 || result=$?
  rm -rf "$probe"
  return "$result"
}
node_status=reused
if ! node_ready; then
  node_status=installed
  case "$(uname -m)" in x86_64) node_arch=x64 ;; aarch64|arm64) node_arch=arm64 ;; *) echo 'Unsupported Node architecture.' >&2; exit 2 ;; esac
  node_download=$(mktemp -d)
  node_dist="https://nodejs.org/dist/latest-v${node_major}.x"
  curl --fail --silent --show-error --location --connect-timeout 30 --max-time 300 "$node_dist/SHASUMS256.txt" -o "$node_download/SHASUMS256.txt"
  node_archive=$(awk -v major="$node_major" -v arch="$node_arch" '$2 ~ ("^node-v" major "[.][0-9]+[.][0-9]+-linux-" arch "[.]tar[.]xz$") {print $2; exit}' "$node_download/SHASUMS256.txt")
  [ -n "$node_archive" ]
  curl --fail --silent --show-error --location --connect-timeout 30 --max-time 300 "$node_dist/$node_archive" -o "$node_download/$node_archive"
  python3 - "$node_download" "$node_archive" <<'PY'
import hashlib, pathlib, sys
directory, name = pathlib.Path(sys.argv[1]), sys.argv[2]
expected = next(line.split()[0] for line in (directory / 'SHASUMS256.txt').read_text().splitlines() if line.split()[1] == name)
if hashlib.sha256((directory / name).read_bytes()).hexdigest() != expected:
    raise SystemExit('Official Node archive checksum mismatch.')
PY
  tar -xJf "$node_download/$node_archive" --strip-components=1 -C "$cloud_root/node"
  node_ready
fi

# Persist the selected tools to agent, login and clean child shells. The owned
# pnpm path below prevents inherited nvm state from masquerading as preparation.
python3 - "$cloud_root" <<'PY'
import pathlib, shlex, sys
root = pathlib.Path(sys.argv[1])
helper = root / 'env.sh'
# Cloud browsers are Playwright's own: no Google Chrome is installed for the
# checks' default `chrome` channel (verification/desktop/scripts/lib/cli.mjs).
helper.write_text('export PATH=' + shlex.quote(str(root / 'node/bin')) + ':' + shlex.quote(str(root / 'just/bin')) + ':"$HOME/.cargo/bin:$PATH"\nexport RUSTUP_TOOLCHAIN=stable\nexport CARGO_NET_GIT_FETCH_WITH_CLI=true\nexport NESSA_BROWSER_CHANNEL=bundled\n')
line = '. ' + shlex.quote(str(helper)) + '\n'
for name in ('.bashrc', '.bash_profile', '.profile'):
    path = pathlib.Path.home() / name
    content = path.read_text() if path.exists() else ''
    if not content.startswith(line):
        content = line + content
    if not content.endswith(line):
        content = content.rstrip('\n') + '\n' + line
    path.write_text(content)
PY
. "$cloud_root/env.sh"
manager=$(node -p 'const p=JSON.parse(require("node:fs").readFileSync("package.json", "utf8")); if(!Object.hasOwn(p,"packageManager") || typeof p.packageManager!=="string") throw new Error("Expected packageManager"); p.packageManager')
case "$manager" in pnpm@*) ;; *) echo 'Expected a pnpm packageManager.' >&2; exit 2 ;; esac
wanted=${manager#pnpm@}
wanted=${wanted%%+*}
# A working version path can coexist with a missing pnpm import worker. Import
# a tiny local tarball into a private store and consume its known module value.
pnpm_ready() {
  [ -x "$cloud_root/node/bin/pnpm" ] &&
    [ "$("$cloud_root/node/bin/pnpm" --version)" = "$wanted" ] || return 1
  local probe
  probe=$(mktemp -d) || exit 2
  python3 - "$probe" <<'PY_PNPM' || { rm -rf "$probe"; exit 2; }
import io, json, pathlib, sys, tarfile
root = pathlib.Path(sys.argv[1])
package = {'name': 'nessa-cloud-readiness', 'version': '1.0.0', 'main': 'index.cjs'}
with tarfile.open(root / 'dependency.tgz', 'w:gz') as archive:
    for name, content in [('package.json', json.dumps(package)), ('index.cjs', 'module.exports = 42\n')]:
        data = content.encode()
        entry = tarfile.TarInfo('package/' + name)
        entry.size, entry.mode = len(data), 0o644
        archive.addfile(entry, io.BytesIO(data))
(root / 'package.json').write_text(json.dumps({'name': 'nessa-cloud-probe', 'private': True, 'dependencies': {'nessa-cloud-readiness': 'file:./dependency.tgz'}}))
PY_PNPM
  local result=0
  (
    cd "$probe" &&
      "$cloud_root/node/bin/pnpm" install --ignore-workspace --offline --ignore-scripts --lockfile=false --store-dir "$probe/store" &&
      node -e 'if (require(process.argv[1]) !== 42) process.exit(1)' "$probe/node_modules/nessa-cloud-readiness"
  ) >/dev/null 2>&1 || result=$?
  rm -rf "$probe"
  return "$result"
}
pnpm_status=reused
if ! pnpm_ready; then
  pnpm_status=installed
  "$cloud_root/node/bin/npm" uninstall --global --prefix "$cloud_root/node" pnpm
  "$cloud_root/node/bin/npm" install --global --prefix "$cloud_root/node" "$manager"
  pnpm_ready
fi
[[ "$(node --version)" = "v${node_major}."* ]]

bash "$cloud_root/native-build-deps.sh" webkit2gtk-driver xvfb gh
# Component metadata is not executable readiness. Exercise the compiler, Cargo,
# formatter and Clippy on private source, without dependencies or shared targets.
rust_ready() {
  local probe
  probe=$(mktemp -d) || exit 2
  mkdir "$probe/src" &&
    printf '[package]\nname = "nessa-cloud-readiness"\nversion = "0.0.0"\nedition = "2024"\n[workspace]\n' > "$probe/Cargo.toml" &&
    printf 'pub fn ready() -> bool {\n    true\n}\n' > "$probe/src/lib.rs" || { rm -rf "$probe"; exit 2; }
  local result=0
  (
    cd "$probe" &&
      rustup run stable cargo check --offline --target-dir "$probe/target" &&
      rustup run stable cargo fmt --check &&
      rustup run stable cargo clippy --offline --target-dir "$probe/target" -- -D warnings
  ) >/dev/null 2>&1 || result=$?
  rm -rf "$probe"
  return "$result"
}
rust_status=reused
if ! rust_ready; then
  rust_status=installed
  # Reinstall from an absent toolchain: install alone trusts component metadata.
  rustup toolchain uninstall stable
  rustup toolchain install stable --profile minimal --component rustfmt --component clippy
  rust_ready
fi
# The public launch recipes, and the script tests that execute them, run `just`.
# Version output is its whole capability: it is one static binary.
just_ready() {
  [ "$("$cloud_root/just/bin/just" --version 2>/dev/null)" = "just $just_version" ]
}
just_status=reused
if ! just_ready; then
  just_status=installed
  cargo install just --locked --force --version "$just_version" --root "$cloud_root/just"
  just_ready
fi
# Invoke the installed canonical owner against the task checkout. Its branch
# may predate the readiness check; manifests and pin still belong to that task.
ui_workspace() {
  node --input-type=module - "$cloud_root/ui-workspace.mjs" "$repository" "$1" <<'JS'
import { pathToFileURL } from 'node:url'
const owner = await import(pathToFileURL(process.argv[2]).href)
const root = process.argv[3]
if (process.argv[4] === 'check') {
  const problem = owner.checkVendor({ root, requireOwned: true })
  if (problem) { console.error(problem); process.exitCode = 1 }
} else {
  owner.prepareVendor({ root })
}
JS
}
ui_status=reused
if ! ui_workspace check; then
  ui_status=prepared
  ui_workspace prepare
fi
dependencies_status=offline
if ! PNPM_CONFIG_OFFLINE=true pnpm install --offline --frozen-lockfile; then
  dependencies_status=fetched
  pnpm install --frozen-lockfile
fi
cargo_status=offline
if ! cargo fetch --offline --locked; then
  cargo_status=fetched
  cargo fetch --locked
fi
browser_status=reused
browsers_ready() {
  node --input-type=module - "$@" <<'JS'
import { accessSync, constants } from 'node:fs'
import * as playwright from 'playwright'
for (const name of process.argv.slice(2)) {
  const engine = playwright[name]
  accessSync(engine.executablePath(), constants.X_OK)
  const browser = await engine.launch({ headless: true, timeout: 15_000 })
  try {
    const page = await browser.newPage()
    await page.setContent('<body>Nessa cloud startup</body>')
    if (await page.textContent('body') !== 'Nessa cloud startup') throw new Error('Browser readiness page did not match.')
  } finally {
    await browser.close()
  }
}
JS
}
# A sandbox whose network refuses Playwright's download host can still ship
# its own Playwright browsers (PLAYWRIGHT_BROWSERS_PATH). The fallback links
# that Chromium under the revision this checkout's Playwright expects, in an
# owned directory, and records the sandbox's original path so a rerun starts
# from it rather than from the links. WebKit has no such stand-in.
browser_links=$cloud_root/browsers
if [ "${PLAYWRIGHT_BROWSERS_PATH:-}" = "$browser_links" ] && [ -f "$cloud_root/browsers-source" ]; then
  PLAYWRIGHT_BROWSERS_PATH=$(cat "$cloud_root/browsers-source")
  if [ -z "$PLAYWRIGHT_BROWSERS_PATH" ]; then unset PLAYWRIGHT_BROWSERS_PATH; fi
fi
browser_source=${PLAYWRIGHT_BROWSERS_PATH:-}
link_sandbox_chromium() {
  rm -rf "$browser_links.tmp"
  mkdir -p "$browser_links.tmp"
  PLAYWRIGHT_BROWSERS_PATH="$browser_links" node --input-type=module - "$browser_source" "$browser_links.tmp" "$browser_links" <<'JS'
import fs from 'node:fs'
import path from 'node:path'
import { chromium } from 'playwright'
const [source, staging, links] = process.argv.slice(2)
const find = (prefix, names) => {
  for (const entry of fs.readdirSync(source).filter(name => name.startsWith(prefix)).sort().reverse())
    for (const name of names) {
      const candidate = path.join(source, entry, name)
      if (fs.existsSync(candidate)) return candidate
    }
  throw new Error(`No ${prefix}* browser under ${source}`)
}
const link = (target, relative) => {
  const at = path.join(staging, relative)
  fs.mkdirSync(path.dirname(at), { recursive: true })
  fs.symlinkSync(target, at)
}
const expected = path.relative(links, chromium.executablePath())
if (expected.startsWith('..')) throw new Error('Playwright ignored PLAYWRIGHT_BROWSERS_PATH.')
const revision = expected.split(path.sep)[0].split('-').at(-1)
link(find('chromium-', ['chrome-linux64/chrome', 'chrome-linux/chrome']), expected)
link(
  find('chromium_headless_shell-', ['chrome-headless-shell-linux64/chrome-headless-shell', 'chrome-linux/headless_shell']),
  `chromium_headless_shell-${revision}/chrome-headless-shell-linux64/chrome-headless-shell`,
)
JS
  rm -rf "$browser_links"
  mv "$browser_links.tmp" "$browser_links"
  printf '%s' "$browser_source" > "$cloud_root/browsers-source"
  export PLAYWRIGHT_BROWSERS_PATH=$browser_links
  printf 'export PLAYWRIGHT_BROWSERS_PATH=%q\n' "$browser_links" >> "$cloud_root/env.sh"
}
if ! browsers_ready chromium webkit; then
  browser_status=installed
  install_status=0
  timeout --kill-after=30s 45m pnpm exec playwright install --with-deps chromium webkit || install_status=$?
  if [ "$install_status" -ne 0 ]; then
    # A stall (124) is not a refused download; only a refusal falls back.
    if [ "$install_status" -eq 124 ] || [ -z "$browser_source" ] || [ ! -d "$browser_source" ]; then
      exit "$install_status"
    fi
    echo "Playwright could not download its browsers; using the sandbox's Chromium from $browser_source. WebKit checks cannot run until the network allows Playwright's download hosts." >&2
    link_sandbox_chromium
    browser_status=sandbox-chromium
  fi
fi
if [ "$browser_status" = sandbox-chromium ]; then
  browsers_ready chromium
else
  browsers_ready chromium webkit
fi
ui_workspace check
node --version
pnpm --version
rustup run stable rustc --version
just --version

# Commit success only after capability checks. An unsuccessful run leaves the
# earlier receipt unchanged, so an old success cannot impersonate this attempt.
python3 - "$cloud_root" "$state" "$mode" "$node_status" "$pnpm_status" "$rust_status" "$just_status" "$ui_status" "$dependencies_status" "$cargo_status" "$browser_status" "$script_file" <<'PY'
import datetime, hashlib, json, pathlib, subprocess, sys
root = pathlib.Path(sys.argv[1])
before = json.loads(pathlib.Path(sys.argv[2]).read_text())
deployment = json.loads((root / 'bootstrap.json').read_text())
receipt = {
    'mode': sys.argv[3],
    'completedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'sourceHead': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'prepareSha256': hashlib.sha256(pathlib.Path(sys.argv[12]).read_bytes()).hexdigest(),
    'bootstrapSourceHead': deployment['sourceHead'],
    'nativeSha256': deployment['hashes']['native-build-deps.sh'],
    'uiSha256': deployment['hashes']['ui-workspace.mjs'],
    **before,
    'steps': dict(zip(('node', 'pnpm', 'rust', 'just', 'ui', 'dependencies', 'cargo', 'browsers'), sys.argv[4:12])),
}
for name, value in (('cache-identity.json', before['cacheIdentity']), ('preparation.json', receipt)):
    temporary = root / (name + '.tmp')
    temporary.write_text(json.dumps(value))
    temporary.replace(root / name)
print(json.dumps(receipt))
PY
