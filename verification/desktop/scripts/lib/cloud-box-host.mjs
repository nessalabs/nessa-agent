/**
 * The stand-in cloud the stress check's `cloudbox` step makes Cloud Boxes
 * in (#1427): a directory the gateway is given as `NESSA_CLOUD_BOX_HOST`
 * (debug builds only), laid out as `LocalBoxHost` in
 * `crates/nessa-conversation/src/workspaces/infrastructure/local_box_host.rs`
 * reads it. Each box is a directory whose commands run on this machine, so
 * the box runs the real bootstrap helper, a real second gateway and the
 * tunnel stand-in.
 */
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { join } from "node:path"

import { repoRoot } from "../../../../scripts/mcp-test-server/local-gateway.mjs"

/** The stage every box's gateway runs in, as the check's own gateway does. */
const BOX_STAGE = "ci"

/**
 * Lay the stand-in cloud out in `directory`: the helper, this checkout's
 * `nessa` and the tunnel stand-in where an image keeps them. Returns its
 * root, for `NESSA_CLOUD_BOX_HOST`.
 */
export function prepareCloudBoxHost(directory, { cloudflared }) {
  const root = join(directory, "cloud-box-host")
  const bin = join(root, "bin")
  mkdirSync(bin, { recursive: true })
  copyFileSync(
    join(repoRoot, "docs/guides/headless-gateway/nessa-gateway-bootstrap"),
    join(bin, "nessa-gateway-bootstrap"),
  )
  chmodSync(join(bin, "nessa-gateway-bootstrap"), 0o755)
  symlinkSync(join(repoRoot, "target/debug/nessa"), join(bin, "nessa"))
  symlinkSync(cloudflared, join(bin, "cloudflared"))
  // The image's gateway.env, pointing the helper at this host's programs.
  writeFileSync(
    join(root, "env"),
    [
      `NESSA_STAGE=${BOX_STAGE}`,
      `NESSA_BIN=${join(bin, "nessa")}`,
      `NESSA_CLOUDFLARED_BIN=${join(bin, "cloudflared")}`,
      "",
    ].join("\n"),
  )
  return root
}

/**
 * What the image leaves in every new box's home: the box gateway's
 * `config.json`, running `agents` (an `agents` block as the check's own
 * gateway is configured, without its MCP servers), so the box keeps
 * conversations and serves terminals.
 */
export function seedBoxConfig(root, agents) {
  const namespace = join(root, "home", ".nessa-box", "gateway", BOX_STAGE)
  mkdirSync(namespace, { recursive: true, mode: 0o700 })
  for (const each of [
    join(root, "home", ".nessa-box"),
    join(root, "home", ".nessa-box", "gateway"),
    namespace,
  ])
    chmodSync(each, 0o700)
  const config = join(namespace, "config.json")
  writeFileSync(config, JSON.stringify({ agents: { ...agents, mcpServers: [] } }), {
    mode: 0o600,
  })
  chmodSync(config, 0o600)
}

/** The lines the stand-in logged: `create <vm>`, `create <vm> lost`, `connect`, `stop`, `remove`. */
export function boxEvents(root) {
  try {
    return readFileSync(join(root, "events.log"), "utf8").split("\n").filter(Boolean)
  } catch {
    return []
  }
}

/** The boxes that exist now, by their VM name (`box-<boxId>`). */
export function boxVms(root) {
  const vms = join(root, "vms")
  return existsSync(vms) ? readdirSync(vms) : []
}

/** Ask the stand-in to lose its next create's answer, after making the VM. */
export function loseNextCreate(root) {
  writeFileSync(join(root, "lose-next-create"), "")
}

/** The desktop credential the box's gateway provisioned for itself. */
export function boxCredential(root, vm) {
  return readFileSync(
    join(
      root,
      "vms",
      vm,
      ".nessa-box",
      "gateway",
      BOX_STAGE,
      "auth",
      "surfaces",
      "nessa-panel.token",
    ),
    "utf8",
  ).trim()
}

/** Whether the box's gateway process the helper started is still running. */
export function boxGatewayRunning(root, vm) {
  try {
    const pid = Number(
      readFileSync(join(root, "vms", vm, ".nessa-box", "gateway.pid"), "utf8").trim(),
    )
    if (!Number.isInteger(pid) || pid <= 1) return false
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

/** End every process any box's helper started, as deleting the boxes would. */
export function endBoxes(root) {
  for (const vm of boxVms(root)) {
    const dir = join(root, "vms", vm, ".nessa-box")
    if (!existsSync(dir)) continue
    for (const name of readdirSync(dir).filter((each) => each.endsWith(".pid"))) {
      const pid = Number(readFileSync(join(dir, name), "utf8").trim())
      if (!Number.isInteger(pid) || pid <= 1) continue
      try {
        // Each runs in a session of its own: end its whole group.
        process.kill(-pid, "SIGTERM")
      } catch {
        // Already ended.
      }
    }
  }
}
