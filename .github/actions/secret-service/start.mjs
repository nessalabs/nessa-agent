import { spawnSync } from "node:child_process"
import { randomBytes } from "node:crypto"
import { appendFileSync, mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

// This VM owns its session bus and unlocked disposable collection. Credentials
// still use the production Secret Service adapter; there is no file-store switch.
const runtime = mkdtempSync(join(tmpdir(), "nessa-secret-service-"))
const bus = spawnSync("dbus-daemon", ["--session", "--fork", "--print-address=1"], {
  encoding: "utf8",
})
if (bus.status !== 0) throw new Error("Could not start session bus")
const address = bus.stdout.trim()
const env = {
  ...process.env,
  HOME: runtime,
  XDG_DATA_HOME: join(runtime, "share"),
  DBUS_SESSION_BUS_ADDRESS: address,
  XDG_RUNTIME_DIR: runtime,
}
const daemon = spawnSync("gnome-keyring-daemon", ["--unlock", "--components=secrets"], {
  env,
  input: randomBytes(32).toString("hex"),
  encoding: "utf8",
  timeout: 30000,
})
if (daemon.status !== 0)
  throw new Error("Could not unlock disposable credential collection")
appendFileSync(
  process.env.GITHUB_ENV,
  `DBUS_SESSION_BUS_ADDRESS=${address}\nXDG_RUNTIME_DIR=${runtime}\nNESSA_TEST_SECRET_SERVICE=disposable\n`,
)
