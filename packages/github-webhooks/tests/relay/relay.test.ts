import assert from "node:assert/strict"
import { test } from "node:test"
import { readFile } from "node:fs/promises"
import { createHmac, randomUUID, webcrypto } from "node:crypto"
import { Miniflare } from "miniflare"
import { D1RelayInbox } from "../../src/relay/infrastructure/d1.js"
import { GitHubIdentityAdapter } from "../../src/relay/infrastructure/github.js"
import { HmacWakeTickets } from "../../src/relay/infrastructure/tickets.js"
import { readRelay } from "../../src/relay/application/read.js"
const secret = "test-webhook-secret-at-least-32-characters"
async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const worker = new Miniflare({
    modules: true,
    modulesRules: [
      { type: "CompiledWasm", include: ["**/*.wasm"] },
      { type: "Text", include: ["**/*.cedar"] },
    ],
    scriptPath: new URL("../../dist/worker.js", import.meta.url).pathname,
    compatibilityDate: "2026-06-16",
    durableObjects: { WAKE_ROOMS: { className: "RepositoryWakeRoom", useSQLite: true } },
    d1Databases: ["DB"],
    bindings: {
      GITHUB_WEBHOOK_SECRET: secret,
      RELAY_TICKET_SECRET: secret,
      RELAY_STAGE: "dev",
    },
    outboundService: async (req: Request) => {
      const credential = req.headers.get("authorization")
      if (credential === "Bearer invalid_token_1234")
        return Response.json({}, { status: 401 })
      if (new URL(req.url).pathname === "/user")
        return Response.json(
          { id: credential === "Bearer user_two_token123" ? 22 : 11 },
          {
            headers: {
              "x-oauth-scopes":
                credential === "Bearer metadata_only_token" ? "read:user" : "repo",
            },
          },
        )
      return credential === "Bearer user_two_token123"
        ? Response.json({}, { status: 404 })
        : Response.json({ id: 42, private: true })
    },
  })
  t.after(() => worker.dispose())
  const db = await worker.getD1Database("DB")
  for (const name of ["0001_deliveries.sql", "0002_relay.sql"]) {
    const schema = await readFile(
      new URL(`../../migrations/${name}`, import.meta.url),
      "utf8",
    )
    await db.exec(
      schema
        .split(/\n\s*\n/)
        .map((sql) => sql.replaceAll("\n", " "))
        .join("\n"),
    )
  }
  const inbox = new D1RelayInbox(db)
  async function deliver(repository = 42, id = randomUUID(), title = "private") {
    const payload = JSON.stringify({
      repository: { id: repository },
      installation: { id: 90 },
      issue: { title },
    })
    const response = await worker.dispatchFetch("https://relay.test/github/webhook", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-github-delivery": id,
        "x-github-event": "issues",
        "x-hub-signature-256":
          "sha256=" + createHmac("sha256", secret).update(payload).digest("hex"),
      },
      body: payload,
    })
    assert.ok(response.ok, await response.text())
    return id
  }
  const headers = {
    Authorization: "Bearer user_one_token123",
    "x-nessa-stage": "dev",
    "x-github-user-id": "11",
  }
  const page = (after = 0, extra = {}) =>
    worker.dispatchFetch(
      `https://relay.test/relay/v1/events?repository=42&after=${after}`,
      { headers: { ...headers, ...extra } },
    )
  return { worker, db, inbox, deliver, page, headers }
}

test("R1/R2/R3: current GitHub access and identity precede repository-isolated disclosure", async (t) => {
  const { deliver, page, db } = await fixture(t)
  await deliver()
  await deliver(43)
  const result = await page()
  assert.equal(result.status, 200)
  const body = (await result.json()) as {
    events: { repositoryId: string }[]
    userId: string
  }
  assert.equal(body.userId, "11")
  assert.deepEqual(
    body.events.map((e) => e.repositoryId),
    ["42"],
  )
  assert.equal(
    (
      await page(0, {
        Authorization: "Bearer user_two_token123",
        "x-github-user-id": "22",
      })
    ).status,
    403,
  )
  assert.equal(
    (await page(0, { Authorization: "Bearer invalid_token_1234" })).status,
    401,
  )
  assert.equal((await page(0, { "x-github-user-id": "22" })).status, 409)
  assert.equal(
    (await page(0, { Authorization: "Bearer metadata_only_token" })).status,
    403,
  )
  assert.equal(
    (await page(0, { Authorization: "Bearer metadata_only_token" })).status,
    403,
  )
  const decisions = await db
    .prepare("SELECT user_id, allowed, rules FROM relay_read_audit ORDER BY audit_id")
    .all<{ user_id: string; allowed: number; rules: string }>()
  assert.ok(
    decisions.results.some(
      (row: { user_id: string; allowed: number; rules: string }) =>
        row.user_id === "11" &&
        row.allowed === 1 &&
        row.rules.includes("github-current-repository-access"),
    ),
  )
  assert.ok(
    decisions.results.some(
      (row: { user_id: string; allowed: number; rules: string }) =>
        row.user_id === "22" && row.allowed === 0,
    ),
  )
})

test("R4/R5/R7: offline replay uses stable ordered identities and a committed cursor", async (t) => {
  const { deliver, page, inbox } = await fixture(t)
  const id = await deliver()
  await deliver(42, id)
  for (let i = 0; i < 9; i++) await deliver()
  const first = (await (await page()).json()) as {
    events: { deliveryId: string }[]
    next: number
  }
  assert.equal(first.events.length, 8)
  assert.equal(first.events[0].deliveryId, id)
  const replay = await inbox.page("42", 0)
  assert.equal(replay.events.length, 8)
  const next = (await (await page(first.next)).json()) as { events: unknown[] }
  assert.equal(next.events.length, 2)
})

test("R10/R15: expiry writes an atomic gap watermark and deletion evidence", async (t) => {
  const { deliver, db, inbox, page } = await fixture(t)
  const id = await deliver()
  await db
    .prepare(
      "UPDATE deliveries SET received_at = '2000-01-01T00:00:00.000Z' WHERE delivery_id = ?",
    )
    .bind(id)
    .run()
  await inbox.expire("2001-01-01T00:00:00.000Z")
  assert.equal((await page()).status, 410)
  const audit = await db
    .prepare("SELECT delivery_id, cause, initiator FROM relay_cleanup_audit")
    .first()
  assert.deepEqual(audit, {
    delivery_id: id,
    cause: "retention_expired",
    initiator: "system:relay-retention",
  })
  assert.equal((await inbox.page("42", 1)).events.length, 0)
})

test(
  "R8/R9/R14: watch before catch-up receives wake-ups; tickets are stage-bound and expiring",
  { timeout: 10000 },
  async (t) => {
    const { worker, page, deliver, headers } = await fixture(t)
    const body = (await (await page()).json()) as { ticket: string }
    const response = await worker.dispatchFetch("https://relay.test/relay/v1/watch", {
      headers: { ...headers, Upgrade: "websocket", "x-nessa-watch-ticket": body.ticket },
    })
    assert.equal(response.status, 101)
    const socket = response.webSocket!
    socket.accept()
    const changed = new Promise((resolve) =>
      socket.addEventListener("message", (e) => resolve(e.data), { once: true }),
    )
    await deliver()
    assert.equal(await changed, '{"type":"changed"}')
    socket.close()
    const tickets = new HmacWakeTickets(
      secret,
      "dev",
      webcrypto.subtle as SubtleCrypto,
      () => 1000,
    )
    const ticket = await tickets.issue("11", "42")
    assert.equal((await tickets.verify(ticket))?.repositoryId, "42")
    assert.equal(await tickets.verify(ticket + "broken"), null)
    assert.equal(
      await new HmacWakeTickets(
        secret,
        "prod",
        webcrypto.subtle as SubtleCrypto,
        () => 1000,
      ).verify(ticket),
      null,
    )
    assert.equal(
      await new HmacWakeTickets(
        secret,
        "dev",
        webcrypto.subtle as SubtleCrypto,
        () => 122000,
      ).verify(ticket),
      null,
    )
  },
)

test("R12: development rejects a production client's stage", async (t) => {
  const { page } = await fixture(t)
  assert.equal((await page(0, { "x-nessa-stage": "prod" })).status, 409)
})

test("GitHub adapter fails closed for slow, malformed, wrong-identity and refused providers", async () => {
  for (const response of [
    Response.json({ id: "11" }),
    new Response("invalid", { status: 200 }),
    Response.json({}, { status: 503 }),
  ]) {
    assert.equal(
      (await new GitHubIdentityAdapter(async () => response).verify("token", "42")).kind,
      "unavailable",
    )
  }
  assert.equal(
    (
      await new GitHubIdentityAdapter(async () => {
        throw new Error("offline")
      }).verify("token", "42")
    ).kind,
    "unavailable",
  )
})

test("application policy refusal prevents inbox reads using injected boundary ports", async () => {
  const result = await readRelay(
    { token: "t", repositoryId: "42", expectedUser: "11", after: 0 },
    {
      identities: {
        verify: async () => ({
          kind: "verified",
          identity: {
            userId: "11",
            repositoryIds: [],
            oauthScopes: [],
            repositoryIsPrivate: true,
          },
        }),
      },
      policy: { read: () => ({ allowed: false, rules: ["denied"] }) },
      inbox: {
        page: async () => {
          throw new Error("must not disclose")
        },
        expire: async () => {},
      },
      audit: { record: async () => {} },
      tickets: { issue: async () => "ticket", verify: async () => null },
    },
  )
  assert.equal(result.kind, "forbidden")
})

test("audit storage failure prevents a successful payload response", async (t) => {
  const { deliver, db, page } = await fixture(t)
  await deliver()
  await db.exec("DROP TABLE relay_read_audit")
  const response = await page()
  assert.equal(response.status, 503)
  assert.equal((await response.text()).includes("private"), false)
})

test("R18: large payloads use byte-bounded pages and explicit continuation", async (t) => {
  const { deliver, page } = await fixture(t)
  const title = "\\".repeat(300000)
  await deliver(42, randomUUID(), title)
  await deliver(42, randomUUID(), title)
  const first = (await (await page()).json()) as {
    events: unknown[]
    next: number
    hasMore: boolean
  }
  assert.equal(first.events.length, 1)
  assert.equal(first.hasMore, true)
  const second = (await (await page(first.next)).json()) as {
    events: unknown[]
    hasMore: boolean
  }
  assert.equal(second.events.length, 1)
  assert.equal(second.hasMore, false)
})

test("an unconfirmed audit write is a failure, not a disclosure receipt", async () => {
  const { D1RelayReadAudit } = await import("../../src/relay/infrastructure/d1.js")
  const statement = {
    bind() {
      return this
    },
    async first<T>(): Promise<T | null> {
      return null
    },
    async all<T>(): Promise<{ results: T[] }> {
      return { results: [] }
    },
  }
  const audit = new D1RelayReadAudit({ prepare: () => statement }, () => new Date(0))
  await assert.rejects(
    audit.record({
      userId: "11",
      repositoryId: "42",
      allowed: true,
      rules: ["test"],
      after: 0,
    }),
    /audit_unconfirmed/,
  )
})
