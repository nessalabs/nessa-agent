#!/usr/bin/env node
/** Real gateway, @nessa/client, attachment route, and deterministic Claude ACP lifecycle. */
import assert from "node:assert/strict"
import { createHash, randomUUID } from "node:crypto"
import { spawnSync } from "node:child_process"
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { WebSocket } from "ws"
import {
  asImageAttachment,
  NessaClient,
  NessaClientConfig,
  NessaConversationMutationError,
} from "@nessa/client"
import { evidenceFor, readEvidence, waitFor } from "./conversation-smoke/evidence.mjs"
import {
  collectCleanupFailures,
  createFixtureSupervisor,
  createLossyProxy,
  reservePort,
  startGateway,
  stopGateway,
} from "./conversation-smoke/runtime.mjs"
import { assertValidTinyPng, tinyPng } from "./conversation-smoke/tiny-png.mjs"

globalThis.WebSocket = WebSocket
const root = dirname(dirname(fileURLToPath(import.meta.url)))
const temporary = mkdtempSync(join(tmpdir(), "nessa-conversation-smoke-"))
const dataRoot = join(temporary, "data")
const workspace = join(temporary, "workspace")
const ownerPath = join(temporary, "owner.token")
const evidencePath = join(workspace, "provider-evidence.jsonl")
const providerStatePath = join(workspace, "provider-state.json")
const fixturePath = join(root, "scripts/conversation-smoke/claude-acp-fixture.mjs")
const catalogPath = join(root, "crates/nessa-sdk/data/models.json")
const defaultBinary = join(
  root,
  process.platform === "win32" ? "target/debug/nessa.exe" : "target/debug/nessa",
)
// CI uses the binary copied beside this script. Isolated local validation names
// its worktree-owned build explicitly so a shared target cannot supply provenance.
const binary = process.env.NESSA_SMOKE_BINARY ?? defaultBinary
assert.ok(existsSync(binary), `gateway binary does not exist: ${binary}`)
const port = await reservePort()
const gatewayUrl = `ws://127.0.0.1:${port}`
const instance = "conversation-smoke"
const env = {
  ...process.env,
  NESSA_DATA_DIR: dataRoot,
  NESSA_INSTANCE: instance,
  NESSA_STAGE: "ci",
  NESSA_PORT: String(port),
  NESSA_HOST: "127.0.0.1",
}
const conversationId = randomUUID()
const sendRequestId = "request-send-answer"
const answerExecutionId = "execution-answer"
const steerExecutionId = "execution-steer"
const cancelExecutionId = "execution-cancel"
const clients = []
let proxy
let gateway
let fixtureSupervisor
let primaryFailure

const connect = async (url) => {
  const client = await NessaClient.connect({
    stage: "ci",
    url,
    role: "surface",
    surface: { kind: "panel", instance },
    client: { id: "conversation-smoke", version: "0.1.0", platform: "node" },
    profile: "product",
    auth: { credential: readFileSync(ownerPath, "utf8").trim() },
    config: new NessaClientConfig({
      reconnect: {
        enabled: true,
        maxAttempts: 10,
        initialDelayMs: 10,
        maxDelayMs: 100,
        jitter: false,
      },
      requestTimeoutMs: 10_000,
    }),
  })
  clients.push(client)
  return client
}

const viewWith = (client, accept, description) => {
  const operation = waitFor(
    () => client.conversation.read(conversationId),
    accept,
    description,
    15_000,
  )
  return proxy ? proxy.guard(operation) : operation
}

try {
  mkdirSync(workspace)
  fixtureSupervisor = await createFixtureSupervisor()
  const initialized = spawnSync(
    binary,
    ["auth", "init", "--local", "--owner-token-file", ownerPath],
    { cwd: root, env, encoding: "utf8" },
  )
  assert.equal(initialized.status, 0, initialized.stderr)
  assert.ok(!initialized.stdout.includes(readFileSync(ownerPath, "utf8").trim()))

  const configPath = join(dataRoot, "ci", "instances", instance, "config.json")
  writeFileSync(
    configPath,
    JSON.stringify({
      agents: {
        catalog: catalogPath,
        workspace,
        mcpServers: [],
        selected: "claude",
        runtimes: {
          claude: {
            command: process.execPath,
            args: [
              fixturePath,
              "provider-evidence.jsonl",
              "provider-state.json",
              String(fixtureSupervisor.port),
              fixtureSupervisor.nonce,
            ],
            model: "claude-haiku-4-5-20251001",
            toolsEnabled: true,
            contextTokens: 100000,
            outputTokens: 4096,
          },
        },
      },
      session: {
        handshakeTimeoutMs: 1000,
        writeTimeoutMs: 1000,
        currentStateIntervalMs: 50,
      },
    }),
    { mode: 0o600 },
  )
  chmodSync(configPath, 0o600)

  gateway = startGateway(binary, root, env, port)
  await gateway.ready()
  const setupClient = await connect(gatewayUrl)
  const authenticated = await setupClient.auth.session()
  assert.equal(authenticated.organizationId, setupClient.productSession.organizationId)

  assert.deepEqual(
    await setupClient.conversation.create({
      conversationId,
      requestId: "request-create",
      agent: "claude",
    }),
    { conversationId },
  )
  const ready = await viewWith(
    setupClient,
    (view) => view.capabilities.imageInput && view.capabilities.steer,
    "Claude image and steering capabilities",
  )
  assert.equal(ready.runtime?.provider, "claude")

  const originalImage = tinyPng
  assertValidTinyPng(originalImage)
  const originalDigest = `sha256:${createHash("sha256").update(originalImage).digest("hex")}`
  const beginning = await setupClient.attachments.begin(
    conversationId,
    { digest: originalDigest, mimeType: "image/png", size: originalImage.length },
    { requestId: "request-attachment" },
  )
  assert.equal(beginning.state, "upload_required")
  const normalizedReference = await setupClient.attachments.upload(beginning.ticket, {
    mimeType: "image/png",
    bytes: new Blob([originalImage], { type: "image/png" }),
  })
  const imageReference = asImageAttachment(normalizedReference)
  assert.ok(imageReference, "normalized upload must produce a message image reference")
  assert.match(imageReference.digest, /^sha256:[0-9a-f]{64}$/)

  setupClient.close()
  proxy = await createLossyProxy(gatewayUrl, sendRequestId)
  const client = await connect(proxy.url)
  const connectionStates = []
  const stopObservingConnection = client.onConnectionStateChange((state) => {
    connectionStates.push(state.status)
  })
  let lostResponse
  try {
    await proxy.guard(
      client.conversation.send(
        conversationId,
        `answer this fixture\nfixtureCorrelation:${answerExecutionId}`,
        [imageReference],
        [],
        { requestId: sendRequestId, executionId: answerExecutionId },
      ),
    )
    assert.fail("the proxy must drop the committed send response")
  } catch (error) {
    proxy.assertHealthy()
    assert.ok(error instanceof NessaConversationMutationError)
    assert.equal(error.requestId, sendRequestId)
    assert.equal(error.executionId, answerExecutionId)
    assert.equal(error.uncertain, true)
    lostResponse = error
  }
  assert.equal(proxy.evidence.droppedResponses, 1)
  await proxy.guard(
    waitFor(
      () => ({ current: client.connectionState, observed: [...connectionStates] }),
      ({ current, observed }) =>
        current.status === "connected" && observed.includes("reconnecting"),
      "automatic client reconnection",
    ),
  )
  stopObservingConnection()
  const promptEvidence = await proxy.guard(
    waitFor(
      () =>
        readEvidence(evidencePath).filter(
          (event) =>
            event.type === "prompt" && event.expectedExecutionId === answerExecutionId,
        ),
      (events) => events.length === 1,
      "one provider prompt after the lost response",
    ),
  )
  const [{ providerSessionId }] = promptEvidence
  assert.ok(promptEvidence[0].promptTypes.includes("image"))
  assert.equal(proxy.evidence.matchingSendFrames, 1, "reconnection must not replay send")

  const recovered = await proxy.guard(lostResponse.retry())
  assert.equal(recovered.requestId, sendRequestId)
  assert.equal(recovered.executionId, answerExecutionId)
  assert.equal(
    proxy.evidence.matchingSendFrames,
    2,
    "only the explicit retry sends again",
  )
  assert.equal(
    evidenceFor(
      readEvidence(evidencePath),
      "prompt",
      providerSessionId,
      answerExecutionId,
    ).length,
    1,
    "same-identity retry must not redispatch to the provider",
  )

  const permissionView = await viewWith(
    client,
    (view) =>
      view.permissions.some((permission) => permission.executionId === answerExecutionId),
    "permission for the original execution",
  )
  const permission = permissionView.permissions.find(
    (candidate) => candidate.executionId === answerExecutionId,
  )
  assert.ok(permission)
  assert.equal(permission.permissionId, `review-${answerExecutionId}`)
  assert.deepEqual(
    permission.options.map((option) => option.id),
    ["allow-once", "deny-once"],
  )

  const steered = await proxy.guard(
    client.conversation.steer(
      conversationId,
      `refine it\nfixtureCorrelation:${steerExecutionId}`,
      [],
      [],
      { requestId: "request-steer", executionId: steerExecutionId },
    ),
  )
  assert.equal(steered.executionId, steerExecutionId)
  assert.equal(steered.disposition, "injected")
  const correlated = await viewWith(
    client,
    (view) =>
      view.messages.some(
        (message) =>
          message.executionId === steerExecutionId &&
          message.steeringTarget === answerExecutionId,
      ),
    "steering correlation to the active execution",
  )
  assert.equal(
    correlated.messages.find((message) => message.executionId === steerExecutionId)
      ?.status,
    "injected",
  )
  assert.equal(
    evidenceFor(
      readEvidence(evidencePath),
      "steer",
      providerSessionId,
      steerExecutionId,
    )[0]?.activeExpectedExecutionId,
    answerExecutionId,
  )

  const answered = await proxy.guard(
    client.conversation.answer(
      conversationId,
      answerExecutionId,
      permission.permissionId,
      "allow-once",
      { requestId: "request-answer" },
    ),
  )
  assert.equal(answered.applied, true)
  await viewWith(
    client,
    (view) =>
      view.messages.some(
        (message) =>
          message.executionId === answerExecutionId && message.status === "completed",
      ),
    "answered execution completion",
  )
  assert.equal(
    evidenceFor(
      readEvidence(evidencePath),
      "permission-answer",
      providerSessionId,
      answerExecutionId,
    )[0]?.optionId,
    "allow-once",
  )

  client.close()
  proxy.assertHealthy()
  await proxy.close()
  proxy = undefined
  await stopGateway(gateway)
  gateway = startGateway(binary, root, env, port)
  await gateway.ready()
  const restarted = await connect(gatewayUrl)
  await restarted.conversation.create({
    conversationId,
    requestId: "request-reopen",
    agent: "claude",
  })
  const restored = await viewWith(
    restarted,
    (view) =>
      view.messages.some(
        (message) =>
          message.executionId === answerExecutionId && message.status === "completed",
      ),
    "persisted conversation after gateway restart",
  )
  assert.deepEqual(
    restored.messages.find((message) => message.executionId === answerExecutionId)
      ?.attachments,
    [imageReference],
  )
  assert.equal(
    restored.messages.filter((message) => message.executionId === answerExecutionId)
      .length,
    1,
  )
  const resumes = evidenceFor(
    readEvidence(evidencePath),
    "session-resume",
    providerSessionId,
    undefined,
  )
  assert.equal(resumes.length, 1)
  assert.equal(
    evidenceFor(
      readEvidence(evidencePath),
      "prompt",
      providerSessionId,
      answerExecutionId,
    ).length,
    1,
  )

  const cancelReceipt = await restarted.conversation.send(
    conversationId,
    `wait for close\nfixtureCorrelation:${cancelExecutionId}`,
    [],
    [],
    { requestId: "request-cancel-execution", executionId: cancelExecutionId },
  )
  assert.equal(cancelReceipt.executionId, cancelExecutionId)
  await viewWith(
    restarted,
    (view) =>
      view.messages.some(
        (message) =>
          message.executionId === cancelExecutionId && message.status === "running",
      ),
    "active execution before close",
  )
  await waitFor(
    () =>
      evidenceFor(
        readEvidence(evidencePath),
        "prompt",
        providerSessionId,
        cancelExecutionId,
      ),
    (events) => events.length === 1,
    "provider prompt before close cancellation",
  )
  assert.equal(
    evidenceFor(
      readEvidence(evidencePath),
      "cancel",
      providerSessionId,
      cancelExecutionId,
    ).length,
    0,
    "the execution is active before close causes cancellation",
  )
  const closed = await restarted.conversation.close(conversationId, {
    requestId: "request-close",
  })
  assert.equal(closed.applied, true)
  const terminal = await viewWith(
    restarted,
    (view) =>
      view.messages.some(
        (message) =>
          message.executionId === cancelExecutionId && message.status === "cancelled",
      ),
    "causal cancelled terminal after close",
  )
  assert.equal(
    terminal.messages.filter((message) => message.executionId === cancelExecutionId)
      .length,
    1,
  )
  assert.equal(
    evidenceFor(
      readEvidence(evidencePath),
      "cancel",
      providerSessionId,
      cancelExecutionId,
    ).length,
    1,
  )
  assert.equal(
    evidenceFor(
      readEvidence(evidencePath),
      "terminal",
      providerSessionId,
      cancelExecutionId,
    )[0]?.stopReason,
    "cancelled",
  )
  const resumedProcessId = resumes[0].processId
  await waitFor(
    () => ({
      ended: readEvidence(evidencePath).some(
        (event) => event.type === "process-end" && event.processId === resumedProcessId,
      ),
      connected: fixtureSupervisor.activeProcessIds().includes(resumedProcessId),
    }),
    ({ ended, connected }) => ended && !connected,
    "provider process cleanup after close",
  )
} catch (error) {
  const logs = gateway?.logs() ?? ""
  const secret = existsSync(ownerPath) ? readFileSync(ownerPath, "utf8").trim() : ""
  const redactedLogs = secret ? logs.replaceAll(secret, "[credential redacted]") : logs
  primaryFailure = new Error(
    `${error?.stack ?? error}\nGateway log tail:\n${redactedLogs}`,
    { cause: error },
  )
}

const cleanupFailures = await collectCleanupFailures([
  ...clients.map((client, index) => ({
    name: `client ${index + 1}`,
    run: () => client.close(),
  })),
  ...(proxy ? [{ name: "lossy proxy", run: () => proxy.close() }] : []),
  ...(fixtureSupervisor
    ? [{ name: "fixture supervisor", run: () => fixtureSupervisor.shutdown() }]
    : []),
  ...(gateway ? [{ name: "gateway", run: () => stopGateway(gateway) }] : []),
])
if (
  !cleanupFailures.some((failure) =>
    ["gateway", "fixture supervisor"].includes(failure.cleanupName),
  )
) {
  cleanupFailures.push(
    ...(await collectCleanupFailures([
      {
        name: "temporary data",
        run: () => rmSync(temporary, { recursive: true, force: true }),
      },
    ])),
  )
}

if (primaryFailure && cleanupFailures.length === 0) throw primaryFailure
if (primaryFailure || cleanupFailures.length > 0)
  throw new AggregateError(
    [...(primaryFailure ? [primaryFailure] : []), ...cleanupFailures],
    primaryFailure
      ? "conversation smoke and cleanup failed"
      : "conversation smoke cleanup failed",
  )
console.log("conversation smoke passed")
