import assert from "node:assert/strict"
import { execFileSync, spawnSync } from "node:child_process"
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const bootstrap = fileURLToPath(
  new URL("../../docs/guides/headless-gateway/nessa-gateway-bootstrap", import.meta.url),
)

/** A box directory and a bin directory of stand-ins, removed after `t`. */
function sandbox(t) {
  const root = mkdtempSync(join(tmpdir(), "nessa-box-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const bin = join(root, "bin")
  mkdirSync(bin)
  const box = join(root, "box")
  const program = (name, body) =>
    writeFileSync(join(bin, name), `#!/bin/sh\n${body}\n`, { mode: 0o755 })
  const run = (command, env = {}) =>
    spawnSync("bash", [bootstrap, command], {
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: bin + ":" + process.env.PATH,
        HOME: root,
        NESSA_BOX_DIR: box,
        NESSA_BIN: join(bin, "nessa"),
        NESSA_CLOUDFLARED_BIN: join(bin, "cloudflared"),
        NESSA_BOOTSTRAP_WAIT_SECONDS: "2",
        ...env,
      },
    })
  return { root, bin, box, program, run }
}

test("a gateway that is not answering is idle only when no process of it runs", (t) => {
  const { box, program, run } = sandbox(t)
  program("curl", "exit 1")
  assert.deepEqual(JSON.parse(run("busy").stdout), { busy: false }, "never started")

  mkdirSync(box, { recursive: true })
  const sleeper = spawnSync("sh", ["-c", "sleep 30 >/dev/null 2>&1 & echo $!"], {
    encoding: "utf8",
  })
  const pid = sleeper.stdout.trim()
  t.after(() => {
    try {
      process.kill(Number(pid))
    } catch {
      // Already ended.
    }
  })
  writeFileSync(join(box, "gateway.pid"), pid + "\n")
  assert.deepEqual(
    JSON.parse(run("busy").stdout),
    { busy: true },
    "started, not answering",
  )

  writeFileSync(join(box, "gateway.pid"), "not-a-pid\n")
  assert.deepEqual(JSON.parse(run("busy").stdout), { busy: false }, "unreadable pid")
})

test("a healthy gateway is busy unless server.activity says plainly that it is not", (t) => {
  const { program, run } = sandbox(t)
  program("curl", "exit 0")
  program(
    "nessa",
    `[ "$1 $2" = "call server.activity" ] || exit 9
[ -n "$FAIL" ] && exit 1
echo "$ACTIVITY"`,
  )
  for (const [activity, busy, fail = ""] of [
    [
      { busy: false, runningConversations: 0, backgroundCommands: 0, openTerminals: 0 },
      false,
    ],
    [
      { busy: true, runningConversations: 1, backgroundCommands: 0, openTerminals: 0 },
      true,
    ],
    [
      { busy: true, runningConversations: 0, backgroundCommands: 0, openTerminals: 2 },
      true,
    ],
    [{ busy: "false" }, true],
    [{}, true],
    ["not json", true],
    [{ busy: false }, true, "1"],
  ]) {
    const result = run("busy", { ACTIVITY: JSON.stringify(activity), FAIL: fail })
    assert.equal(result.status, 0, result.stderr)
    assert.deepEqual(
      JSON.parse(result.stdout),
      { busy },
      JSON.stringify([activity, fail]),
    )
  }
})

test("create starts both processes and prints the app tunnel and the desktop credential", (t) => {
  const { root, box, program, run } = sandbox(t)
  const data = join(box, "gateway")
  // The stand-in server provisions its desktop surface credential on start,
  // as `nessa server --provision-local` does, and records how it was started.
  program(
    "nessa",
    `[ "$1" = server ] || exit 9
mkdir -p "$NESSA_DATA_DIR/auth/surfaces"
printf 'desktop-secret\\n' > "$NESSA_DATA_DIR/auth/surfaces/nessa-panel.token"
printf '%s\\n' "$*" > "${root}/server-args"
touch "${root}/healthy"
sleep 5`,
  )
  program("cloudflared", `printf '%s\\n' "$*" > "${root}/tunnel-args"\nsleep 5`)
  program(
    "curl",
    `case "$*" in
  *"/health"*"127.0.0.1"*|*"127.0.0.1"*"/health"*) [ -e "${root}/healthy" ] ;;
  *"/quicktunnel"*) echo '{"hostname":"box-1.trycloudflare.com"}' ;;
  *"https://box-1.trycloudflare.com/health"*) exit 0 ;;
  *) exit 1 ;;
esac`,
  )
  const result = run("create")
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(JSON.parse(result.stdout), {
    url: "wss://box-1.trycloudflare.com",
    credential: "desktop-secret",
  })
  assert.match(
    readFileSync(join(root, "server-args"), "utf8"),
    new RegExp(
      `server --provision-local --vault-passphrase-file ${join(box, "vault-passphrase")}`,
    ),
  )
  assert.match(
    readFileSync(join(root, "tunnel-args"), "utf8"),
    /tunnel --no-autoupdate --metrics 127\.0\.0\.1:20241 --url http:\/\/127\.0\.0\.1:7420/,
  )
  assert.equal(statSync(join(box, "vault-passphrase")).mode & 0o777, 0o600)
  assert.ok(readFileSync(join(box, "vault-passphrase"), "utf8").trim().length >= 40)
  assert.ok(statSync(data).isDirectory())

  const status = run("status")
  assert.equal(status.status, 0, status.stderr)
  assert.deepEqual(JSON.parse(status.stdout), { url: "wss://box-1.trycloudflare.com" })
})

test("create fails, printing nothing, when the tunnel never serves the gateway", (t) => {
  const { root, program, run } = sandbox(t)
  program("nessa", `touch "${root}/healthy"\nsleep 5`)
  program("cloudflared", "sleep 5")
  program(
    "curl",
    `case "$*" in
  *"127.0.0.1:7420/health"*) [ -e "${root}/healthy" ] ;;
  *) exit 1 ;;
esac`,
  )
  const result = run("create")
  assert.notEqual(result.status, 0)
  assert.equal(result.stdout, "")
  assert.match(result.stderr, /app tunnel did not serve this gateway/)
})

test("the helper refuses an unknown command", () => {
  assert.throws(() => execFileSync("bash", [bootstrap, "nap"], { stdio: "pipe" }))
})
