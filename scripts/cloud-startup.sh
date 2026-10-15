#!/usr/bin/env bash
# Setup and maintenance use this same capability reconciliation. Receipts describe
# observed reuse; they do not claim control over the platform's cache scheduler.
# Cold/warm/failure and inherited-tool regressions: cloud-startup.test.mjs.
set -euo pipefail
umask 077
[ "$#" -eq 2 ] || { echo 'Usage: cloud-startup.sh setup|maintenance <checkout>' >&2; exit 2; }
mode=$1
node_major=24
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
    'previousPreparation': {key: prior[key] for key in ('mode', 'completedAt', 'sourceHead', 'prepareSha256')} if prior else None,
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
node_status=reused
if [ ! -x "$cloud_root/node/bin/node" ] || [[ "$("$cloud_root/node/bin/node" --version)" != "v${node_major}."* ]]; then
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
fi

# Persist the selected tools to agent, login and clean child shells. The owned
# pnpm path below prevents inherited nvm state from masquerading as preparation.
python3 - "$cloud_root" <<'PY'
import pathlib, shlex, sys
root = pathlib.Path(sys.argv[1])
helper = root / 'env.sh'
helper.write_text('export PATH=' + shlex.quote(str(root / 'node/bin')) + ':"$HOME/.cargo/bin:$PATH"\nexport RUSTUP_TOOLCHAIN=stable\nexport CARGO_NET_GIT_FETCH_WITH_CLI=true\n')
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
pnpm_status=reused
if [ ! -x "$cloud_root/node/bin/pnpm" ] || [ "$("$cloud_root/node/bin/pnpm" --version)" != "$wanted" ]; then
  pnpm_status=installed
  npm install --global --prefix "$cloud_root/node" "$manager"
fi
[ "$("$cloud_root/node/bin/pnpm" --version)" = "$wanted" ]
[[ "$(node --version)" = "v${node_major}."* ]]

bash "$cloud_root/native-build-deps.sh" webkit2gtk-driver xvfb gh
rust_status=reused
components=$(rustup component list --toolchain stable --installed 2>/dev/null || true)
if ! rustup run stable rustc --version >/dev/null 2>&1 || [[ "$components" != *rustfmt-* ]] || [[ "$components" != *clippy-* ]]; then
  rust_status=installed
  rustup toolchain install stable --profile minimal --component rustfmt --component clippy
fi
# Invoke the installed canonical owner against the task checkout. Its branch
# may predate the readiness check; manifests and pin still belong to that task.
ui_workspace() {
  node --input-type=module - "$cloud_root/ui-workspace.mjs" "$repository" "$1" <<'JS'
import { pathToFileURL } from 'node:url'
const owner = await import(pathToFileURL(process.argv[2]).href)
const root = process.argv[3]
if (process.argv[4] === 'check') {
  const problem = owner.checkVendor({ root })
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
  node --input-type=module <<'JS'
import { accessSync, constants } from 'node:fs'
import { chromium, webkit } from 'playwright'
for (const engine of [chromium, webkit]) {
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
if ! browsers_ready; then
  browser_status=installed
  timeout --kill-after=30s 45m pnpm exec playwright install --with-deps chromium webkit
fi
browsers_ready
ui_workspace check
node --version
pnpm --version
rustup run stable rustc --version

# Commit success only after capability checks. An unsuccessful run leaves the
# earlier receipt unchanged, so an old success cannot impersonate this attempt.
python3 - "$cloud_root" "$state" "$mode" "$node_status" "$pnpm_status" "$rust_status" "$ui_status" "$dependencies_status" "$cargo_status" "$browser_status" "$script_file" <<'PY'
import datetime, hashlib, json, pathlib, subprocess, sys
root = pathlib.Path(sys.argv[1])
before = json.loads(pathlib.Path(sys.argv[2]).read_text())
deployment = json.loads((root / 'bootstrap.json').read_text())
receipt = {
    'mode': sys.argv[3],
    'completedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'sourceHead': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'prepareSha256': hashlib.sha256(pathlib.Path(sys.argv[11]).read_bytes()).hexdigest(),
    'bootstrapSourceHead': deployment['sourceHead'],
    'nativeSha256': deployment['hashes']['native-build-deps.sh'],
    'uiSha256': deployment['hashes']['ui-workspace.mjs'],
    **before,
    'steps': dict(zip(('node', 'pnpm', 'rust', 'ui', 'dependencies', 'cargo', 'browsers'), sys.argv[4:11])),
}
for name, value in (('cache-identity.json', before['cacheIdentity']), ('preparation.json', receipt)):
    temporary = root / (name + '.tmp')
    temporary.write_text(json.dumps(value))
    temporary.replace(root / name)
print(json.dumps(receipt))
PY
