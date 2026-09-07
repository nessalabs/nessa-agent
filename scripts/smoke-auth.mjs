/** Real Rust gateway + NessaClient lifecycle, isolated in a temporary local data root. */
import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { once } from "node:events"
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { createServer } from "node:net"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"
import { WebSocket } from "ws"
import { NessaClient, NessaRpcError } from "@nessa/client"

globalThis.WebSocket = WebSocket
const root = fileURLToPath(new URL("../", import.meta.url))
const directory = mkdtempSync(join(tmpdir(), "nessa-auth-e2e-"))
const binary = join(
  root,
  process.platform === "win32"
    ? "target/debug/nessa-server.exe"
    : "target/debug/nessa-server",
)
const listener = createServer().listen(0, "127.0.0.1")
await once(listener, "listening")
const port = listener.address().port
await new Promise((resolve) => listener.close(resolve))
const env = {
  ...process.env,
  NESSA_DATA_DIR: join(directory, "data"),
  NESSA_INSTANCE: "e2e",
  NESSA_STAGE: "ci",
  NESSA_PORT: String(port),
  NESSA_HOST: "127.0.0.1",
}
process.env.NESSA_DATA_DIR = env.NESSA_DATA_DIR
process.env.NESSA_INSTANCE = env.NESSA_INSTANCE
const url = `ws://127.0.0.1:${port}`
let server
const clients = []
let logs = ""
const ownerPath = join(directory, "owner.token")
const options = {
  stage: "ci",
  url,
  role: "surface",
  surface: { kind: "cli", instance: "e2e" },
  client: { id: "e2e", version: "0.1.0", platform: "node" },
}
const connect = async (credential) => {
  const client = await NessaClient.connect({
    ...options,
    profile: "product",
    auth: { credential },
  })
  clients.push(client)
  return client
}
async function stop() {
  if (!server || server.exitCode !== null) return
  const exited = once(server, "exit")
  server.kill("SIGTERM")
  await exited
}
async function start() {
  server = spawn(binary, [], { cwd: root, env, stdio: ["ignore", "pipe", "pipe"] })
  server.stdout.on("data", (bytes) => {
    logs += bytes.toString()
  })
  server.stderr.on("data", (bytes) => {
    logs += bytes.toString()
  })
  for (let attempt = 0; attempt < 100; attempt++) {
    if (server.exitCode !== null) throw new Error(`gateway startup failed: ${logs}`)
    try {
      if ((await fetch(`http://127.0.0.1:${port}/health`)).ok) return
    } catch {
      /* starting */
    }
    await sleep(50)
  }
  throw new Error("gateway startup timed out")
}
async function expectClose(client, action, expectedReason) {
  let off
  const closed = new Promise((resolve) => {
    off = client.onClose(resolve)
  })
  try {
    await action()
    const error = await Promise.race([
      closed,
      sleep(7000).then(() => {
        throw new Error("revoked/expired socket stayed open")
      }),
    ])
    assert.equal(error.closeReason, expectedReason)
    assert.equal(error.retryable, false)
    assert.equal(client.connectionState.status, "closed")
  } finally {
    off()
  }
}
try {
  const init = spawnSync(binary, ["auth", "init", "--owner-token-file", ownerPath], {
    env,
    encoding: "utf8",
  })
  assert.equal(init.status, 0, init.stderr)
  const ownerSecret = readFileSync(ownerPath, "utf8").trim()
  if (process.platform !== "win32") assert.equal(statSync(ownerPath).mode & 0o777, 0o600)
  assert.ok(!init.stdout.includes(ownerSecret))
  const duplicatePath = join(directory, "duplicate-owner.token")
  const duplicateInit = spawnSync(
    binary,
    ["auth", "init", "--owner-token-file", duplicatePath],
    { env, encoding: "utf8" },
  )
  assert.notEqual(duplicateInit.status, 0)
  assert.equal(existsSync(duplicatePath), false)
  const overwrite = spawnSync(binary, ["auth", "init", "--owner-token-file", ownerPath], {
    env,
    encoding: "utf8",
  })
  assert.notEqual(overwrite.status, 0)
  assert.equal(readFileSync(ownerPath, "utf8").trim(), ownerSecret)
  const configPath = join(env.NESSA_DATA_DIR, "ci", "instances", "e2e", "config.json")
  // Offline commands and serving use the same file; invalid values never default.
  writeFileSync(configPath, JSON.stringify({ registry: { maxCredentials: 0 } }))
  const invalidConfig = spawnSync(
    binary,
    [
      "auth",
      "recover-owner",
      "--owner-token-file",
      join(directory, "invalid-config.token"),
    ],
    { env, encoding: "utf8" },
  )
  assert.notEqual(invalidConfig.status, 0)
  assert.equal(existsSync(join(directory, "invalid-config.token")), false)
  writeFileSync(
    configPath,
    JSON.stringify({
      registry: { maxCredentials: 2000, maxRegistryBytes: 8388608 },
      session: {
        handshakeTimeoutMs: 1000,
        writeTimeoutMs: 500,
        currentStateIntervalMs: 100,
      },
    }),
  )
  await start()
  assert.equal((await fetch(`http://127.0.0.1:${port}/`)).status, 404)
  const idle = new WebSocket(`${url}/session`)
  const idleClosed = once(idle, "close")
  const deadlineStart = Date.now()
  const [idleCode, idleReason] = await idleClosed
  assert.equal(JSON.parse(idleReason.toString()).code, "handshake_timeout")
  assert.equal(idleCode, 4006)
  assert.ok(
    Date.now() - deadlineStart < 3000,
    "file-configured handshake deadline was not applied",
  )
  const locked = spawnSync(
    binary,
    ["auth", "recover-owner", "--owner-token-file", join(directory, "blocked.token")],
    { env, encoding: "utf8" },
  )
  assert.notEqual(locked.status, 0)
  const owner = await connect(ownerSecret)
  const identity = owner.productSession
  assert.equal(identity.expiresAt, null)
  const chat = await NessaClient.connect({
    ...options,
    profile: "product",
    client: { ...options.client, id: "nessa-panel" },
  })
  clients.push(chat)
  assert.equal(chat.productSession.expiresAt, null)
  assert.notEqual(chat.productSession.credentialId, identity.credentialId)
  assert.notEqual(chat.productSession.principalId, identity.principalId)
  assert.equal((await chat.server.health()).ok, true)
  assert.deepEqual(await chat.conversation.echo("local chat"), { text: "local chat" })
  assert.ok((await chat.credentials.list()).credentials.length >= 2)
  const request = {
    requestId: "reader-issue",
    principal: { id: "reader", kind: "integration" },
    membership: {
      id: "reader-membership",
      principalId: "reader",
      organizationId: identity.organizationId,
      role: "member",
      state: "active",
    },
    expiresAt: Math.floor(Date.now() / 1000) + 3600,
    grants: [
      {
        action: "server.read",
        resource: { organizationId: identity.organizationId, id: identity.gatewayId },
      },
    ],
  }
  const issued = await owner.credentials.issue(request)
  assert.ok("secret" in issued)
  const retry = await owner.credentials.issue(request)
  assert.equal(retry.credential.id, issued.credential.id)
  assert.equal(retry.secretUnavailable, true)
  assert.ok(!("secret" in retry))
  await assert.rejects(
    owner.credentials.issue({ ...request, expiresAt: request.expiresAt + 1 }),
  )
  await assert.rejects(
    owner.credentials.issue({
      ...request,
      requestId: "reject-admin-membership",
      membership: { ...request.membership, role: "admin" },
    }),
  )
  await assert.rejects(
    owner.credentials.issue({
      ...request,
      requestId: "reject-admin-grant",
      grants: [{ ...request.grants[0], action: "credential.manage" }],
    }),
  )
  const permanent = await owner.credentials.issue({
    ...request,
    requestId: "non-expiring",
    expiresAt: undefined,
  })
  assert.equal(permanent.credential.expiresAt, null)
  const permanentClient = await connect(permanent.secret)
  assert.equal(permanentClient.productSession.expiresAt, null)
  const longer = await owner.credentials.issue({
    ...request,
    requestId: "longer-than-month",
    expiresAt: Math.floor(Date.now() / 1000) + 365 * 24 * 60 * 60,
  })
  assert.ok(
    longer.credential.expiresAt > Math.floor(Date.now() / 1000) + 30 * 24 * 60 * 60,
  )
  const reader = await connect(issued.secret)
  const sharedPeer = await connect(issued.secret)
  const busy = await connect(issued.secret)
  const pending = Promise.allSettled(
    Array.from({ length: 1000 }, () => busy.server.health()),
  )
  busy.close()
  const settled = await pending
  assert.equal(settled.length, 1000)
  assert.ok(settled.every((result) => result.status === "rejected"))
  // Closing one connection does not revoke its credential or close its peers.
  assert.equal(sharedPeer.connectionState.status, "connected")
  assert.equal((await sharedPeer.server.health()).ok, true)
  assert.equal((await owner.server.health()).ok, true)
  assert.equal((await permanentClient.server.health()).ok, true)
  assert.equal((await reader.server.health()).ok, true)
  await assert.rejects(
    reader.conversation.echo("denied"),
    (error) => error.code === "forbidden",
  )
  await assert.rejects(
    reader.credentials.list(),
    (error) => error instanceof NessaRpcError && error.code === "forbidden",
  )
  await assert.rejects(connect(`${issued.secret}invalid`))
  const listed = await owner.credentials.list()
  assert.equal(listed.credentials.length, 5)
  assert.ok(!JSON.stringify(listed).includes(issued.secret))

  // Credentials are required; arbitrary strings cannot authenticate.
  await assert.rejects(
    NessaClient.connect({
      ...options,
      auth: { credential: "" },
    }),
  )
  await assert.rejects(connect("invalid-credential"))

  // Pre-auth RPCs cannot reach handlers.
  for (const path of ["/session"]) {
    const socket = new WebSocket(`${url}${path}`)
    const challengePromise = once(socket, "message")
    await once(socket, "open")
    const [challenge] = await challengePromise
    assert.equal(JSON.parse(challenge.toString()).event, "session.challenge")
    const response = once(socket, "message")
    socket.send(
      JSON.stringify({ type: "req", id: "preauth", method: "server.health", params: {} }),
    )
    const [denied] = await response
    assert.equal(JSON.parse(denied.toString()).error.code, "authentication_required")
    socket.close()
  }

  await Promise.all([
    expectClose(sharedPeer, async () => {}, "credential_revoked"),
    expectClose(
      reader,
      () => owner.credentials.revoke(issued.credential.id, "revoke-reader"),
      "credential_revoked",
    ),
  ])
  // Revocation affects every connection using that credential, not other tokens.
  assert.equal((await owner.server.health()).ok, true)
  assert.equal((await permanentClient.server.health()).ok, true)
  await stop()
  await start()
  await assert.rejects(connect(issued.secret))
  const restartedOwner = await connect(ownerSecret)
  assert.equal(restartedOwner.productSession.organizationId, identity.organizationId)
  // Exercise the administrative executable through the same SDK, including protected delivery.
  const cli = (...args) =>
    spawnSync(
      process.execPath,
      [
        "--import",
        "tsx",
        "scripts/nessa-auth.mjs",
        ...args,
        "--url",
        url,
        "--credential-file",
        ownerPath,
      ],
      { cwd: root, encoding: "utf8" },
    )
  const cliList = cli("list")
  assert.equal(cliList.status, 0, cliList.stderr)
  assert.equal(JSON.parse(cliList.stdout).credentials.length, 5)
  const inputPath = join(directory, "issue.json")
  const cliTokenPath = join(directory, "cli-reader.token")
  writeFileSync(inputPath, JSON.stringify({ ...request, requestId: "cli-reader" }))
  const cliIssued = cli("issue", "--input", inputPath, "--out", cliTokenPath)
  assert.equal(cliIssued.status, 0, cliIssued.stderr)
  const cliMetadata = JSON.parse(cliIssued.stdout).credential
  const cliSecret = readFileSync(cliTokenPath, "utf8").trim()
  if (process.platform !== "win32")
    assert.equal(statSync(cliTokenPath).mode & 0o777, 0o600)
  assert.ok(!cliIssued.stdout.includes(cliSecret))
  const cliReader = await connect(cliSecret)
  await expectClose(
    cliReader,
    async () => {
      const revoked = cli("revoke", cliMetadata.id)
      assert.equal(revoked.status, 0, revoked.stderr)
    },
    "credential_revoked",
  )
  const expiring = await restartedOwner.credentials.issue({
    ...request,
    requestId: "short-lived",
    expiresAt: Math.floor(Date.now() / 1000) + 3,
  })
  const shortClient = await connect(expiring.secret)
  await expectClose(shortClient, async () => {}, "credential_expired")
  const registryPath = join(directory, "data/ci/instances/e2e/auth/credentials.v1.json")
  const registry = readFileSync(registryPath, "utf8")
  for (const secret of [ownerSecret, issued.secret, expiring.secret]) {
    assert.ok(!registry.includes(secret))
    assert.ok(!logs.includes(secret))
  }
  await stop()
  const recoveredPath = join(directory, "recovered.token")
  const recovery = spawnSync(
    binary,
    ["auth", "recover-owner", "--owner-token-file", recoveredPath],
    { env, encoding: "utf8" },
  )
  assert.equal(recovery.status, 0, recovery.stderr)
  await start()
  await assert.rejects(connect(ownerSecret))
  const recovered = await connect(readFileSync(recoveredPath, "utf8").trim())
  assert.equal(recovered.productSession.organizationId, identity.organizationId)
  assert.equal((await recovered.server.health()).ok, true)
  await expectClose(
    recovered,
    async () => {
      const revoked = await recovered.credentials.revoke(
        recovered.productSession.credentialId,
        "revoke-self",
      )
      assert.equal(revoked.credentialId, recovered.productSession.credentialId)
    },
    "credential_revoked",
  )
  console.log(
    "auth e2e passed: bootstrap, issue/retry, isolation, denial, idle revocation/expiry, restart, recovery, session isolation, shared-credential revocation, self-revocation acknowledgement, unauthenticated bypass rejection",
  )
} finally {
  for (const client of clients) client.close()
  await stop()
  rmSync(directory, { recursive: true, force: true })
}
