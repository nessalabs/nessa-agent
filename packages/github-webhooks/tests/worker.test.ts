import assert from "node:assert/strict"
import { createHash, createHmac, randomUUID, webcrypto } from "node:crypto"
import { readFile } from "node:fs/promises"
import { test } from "node:test"
import { Miniflare } from "miniflare"
import { inboxLimits } from "../src/webhooks/domain/delivery.js"
import { createWebhookHandler } from "../src/webhooks/infrastructure/http.js"
import { githubPayloadVerifier } from "../src/webhooks/infrastructure/signature.js"

const secret = "local-test-secret-with-at-least-32-characters"
const schema = await readFile(
  new URL("../migrations/0001_deliveries.sql", import.meta.url),
  "utf8",
)

function signed(payload: string, id = randomUUID(), event = "issues") {
  return {
    "content-type": "application/json",
    "x-github-event": event,
    "x-github-delivery": id,
    "x-hub-signature-256":
      "sha256=" + createHmac("sha256", secret).update(payload).digest("hex"),
  }
}

async function fixture(t: { after(fn: () => Promise<void>): void }, configured = true) {
  const worker = new Miniflare({
    modules: true,
    scriptPath: new URL("../dist/worker.js", import.meta.url).pathname,
    compatibilityDate: "2026-06-16",
    d1Databases: ["DB"],
    bindings: configured ? { GITHUB_WEBHOOK_SECRET: secret } : {},
  })
  t.after(() => worker.dispose())
  const db = await worker.getD1Database("DB")
  // Each migration statement is separated by a blank line; trigger bodies stay intact.
  await db.exec(
    schema
      .split(/\n\s*\n/)
      .map((sql) => sql.replaceAll("\n", " "))
      .join("\n"),
  )
  return { worker, db }
}

test("a signed webhook is retained before acceptance, with its complete payload", async (t) => {
  const { worker, db } = await fixture(t)
  const payload = JSON.stringify({
    action: "opened",
    installation: { id: 42 },
    sender: { id: 7 },
    issue: { title: "hello" },
  })
  const headers = signed(payload)
  const result = await worker.dispatchFetch("https://service.test/github/webhook", {
    method: "POST",
    headers,
    body: payload,
  })
  assert.equal(result.status, 202)
  const stored = await db
    .prepare("SELECT * FROM deliveries WHERE delivery_id = ?")
    .bind(headers["x-github-delivery"])
    .first()
  assert.equal(stored?.payload, payload)
  assert.equal(stored?.event, "issues")
  assert.equal(typeof stored?.received_at, "string")
  assert.deepEqual(
    await db.prepare("SELECT deliveries, bytes FROM inbox_usage").first(),
    { deliveries: 1, bytes: Buffer.byteLength(payload) },
  )
})

test("missing, wrong and changed-payload signatures cannot persist deliveries", async (t) => {
  const { worker, db } = await fixture(t)
  for (const signature of [
    "",
    "sha256=" + "0".repeat(64),
    signed("{}")["x-hub-signature-256"],
  ]) {
    const result = await worker.dispatchFetch("https://service.test/github/webhook", {
      method: "POST",
      headers: { ...signed('{"changed":true}'), "x-hub-signature-256": signature },
      body: '{"changed":true}',
    })
    assert.equal(result.status, 401)
  }
  assert.equal(
    (await db.prepare("SELECT COUNT(*) AS count FROM deliveries").first())?.count,
    0,
  )
})

test("concurrent identical redeliveries retain one receipt and acknowledge duplicates", async (t) => {
  const { worker, db } = await fixture(t)
  const payload = '{"action":"created"}'
  const headers = signed(payload, randomUUID(), "issue_comment")
  const results = await Promise.all(
    Array.from({ length: 12 }, () =>
      worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers,
        body: payload,
      }),
    ),
  )
  assert.equal(results.filter((response) => response.status === 202).length, 1)
  assert.equal(results.filter((response) => response.status === 200).length, 11)
  assert.equal(
    (await db.prepare("SELECT deliveries FROM inbox_usage").first())?.deliveries,
    1,
  )
})

test("a reused delivery ID cannot replace its payload or event", async (t) => {
  const { worker, db } = await fixture(t)
  const id = randomUUID()
  for (const [payload, event, status] of [
    ["{}", "issues", 202],
    ['{"other":1}', "issues", 409],
    ["{}", "pull_request", 409],
  ] as const) {
    const result = await worker.dispatchFetch("https://service.test/github/webhook", {
      method: "POST",
      headers: signed(payload, id, event),
      body: payload,
    })
    assert.equal(result.status, status)
  }
  assert.equal(
    (await db.prepare("SELECT payload FROM deliveries").first())?.payload,
    "{}",
  )
})

test("concurrent deliveries cannot exceed the last free row", async (t) => {
  const { worker, db } = await fixture(t)
  const headers = signed("{}")
  await worker.dispatchFetch("https://service.test/github/webhook", {
    method: "POST",
    headers,
    body: "{}",
  })
  await db
    .prepare(
      `WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < ?)
    INSERT INTO deliveries (delivery_id, event, payload_sha256, payload, received_at, bytes)
    SELECT printf('%08x-1111-2222-3333-444444444444', x), 'issues', ?, '{}', '2026-10-25T00:00:00Z', 2 FROM n`,
    )
    .bind(inboxLimits.deliveries - 2, createHash("sha256").update("{}").digest("hex"))
    .run()
  const results = await Promise.all(
    Array.from({ length: 2 }, () =>
      worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers: signed("{}"),
        body: "{}",
      }),
    ),
  )
  assert.deepEqual(results.map((result) => result.status).sort(), [202, 503])
  assert.equal(
    (await db.prepare("SELECT COUNT(*) AS count FROM deliveries").first())?.count,
    inboxLimits.deliveries,
  )
  assert.equal(
    (
      await worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers,
        body: "{}",
      })
    ).status,
    200,
  )
})

test("the byte ceiling refuses new deliveries without evicting accepted data", async (t) => {
  const { worker, db } = await fixture(t)
  const headers = signed("{}")
  await worker.dispatchFetch("https://service.test/github/webhook", {
    method: "POST",
    headers,
    body: "{}",
  })
  const payload = JSON.stringify({ data: "x".repeat(inboxLimits.bodyBytes - 11) })
  const insert = (body: string) =>
    db
      .prepare(
        `INSERT INTO deliveries
    (delivery_id, event, payload_sha256, payload, received_at, bytes) VALUES (?, 'issues', ?, ?, '2026-10-25T00:00:00Z', ?)`,
      )
      .bind(
        randomUUID(),
        createHash("sha256").update(body).digest("hex"),
        body,
        Buffer.byteLength(body),
      )
      .run()
  for (let index = 0; index < 63; index++) await insert(payload)
  await insert(JSON.stringify({ data: "x".repeat(inboxLimits.bodyBytes - 13) }))
  assert.equal(
    (await db.prepare("SELECT bytes FROM inbox_usage").first())?.bytes,
    inboxLimits.storedBytes,
  )
  assert.equal(
    (
      await worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers: signed("{}"),
        body: "{}",
      })
    ).status,
    503,
  )
  assert.equal(
    (
      await worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers,
        body: "{}",
      })
    ).status,
    200,
  )
  assert.equal(
    (await db.prepare("SELECT COUNT(*) AS count FROM deliveries").first())?.count,
    65,
  )
})

test("malformed headers, JSON and unsupported routes fail without storage", async (t) => {
  const { worker, db } = await fixture(t)
  for (const payload of ["{", "[]", "null", '"text"']) {
    assert.equal(
      (
        await worker.dispatchFetch("https://service.test/github/webhook", {
          method: "POST",
          headers: signed(payload),
          body: payload,
        })
      ).status,
      400,
    )
  }
  assert.equal(
    (
      await worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers: { ...signed("{}"), "x-github-delivery": "not-a-uuid" },
        body: "{}",
      })
    ).status,
    400,
  )
  assert.equal(
    (
      await worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers: { ...signed("{}"), "content-type": "text/plain" },
        body: "{}",
      })
    ).status,
    415,
  )
  assert.equal(
    (await worker.dispatchFetch("https://service.test/github/webhook")).status,
    405,
  )
  assert.equal(
    (await worker.dispatchFetch("https://service.test/deliveries")).status,
    404,
  )
  assert.equal((await worker.dispatchFetch("https://service.test/health")).status, 200)
  assert.equal(
    (await db.prepare("SELECT COUNT(*) AS count FROM deliveries").first())?.count,
    0,
  )
})

test("a correctly signed body with invalid UTF-8 is a malformed delivery", async (t) => {
  const { worker, db } = await fixture(t)
  const payload = Buffer.from([0xff])
  const headers = {
    ...signed("{}"),
    "x-hub-signature-256":
      "sha256=" + createHmac("sha256", secret).update(payload).digest("hex"),
  }
  assert.equal(
    (
      await worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers,
        body: payload,
      })
    ).status,
    400,
  )
  assert.equal(
    (await db.prepare("SELECT COUNT(*) AS count FROM deliveries").first())?.count,
    0,
  )
})

test("oversize bodies are refused even without a Content-Length header", async (t) => {
  const { worker } = await fixture(t)
  const body = JSON.stringify({ content: "x".repeat(inboxLimits.bodyBytes) })
  assert.equal(
    (
      await worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers: signed(body),
        body,
      })
    ).status,
    413,
  )
})

test("missing configuration and failed database writes never claim acceptance", async (t) => {
  const { worker, db } = await fixture(t, false)
  assert.equal(
    (
      await worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers: signed("{}"),
        body: "{}",
      })
    ).status,
    503,
  )
  const configured = await fixture(t)
  await configured.db.exec("DROP TABLE deliveries")
  assert.equal(
    (
      await configured.worker.dispatchFetch("https://service.test/github/webhook", {
        method: "POST",
        headers: signed("{}"),
        body: "{}",
      })
    ).status,
    503,
  )
  assert.equal(
    (await db.prepare("SELECT COUNT(*) AS count FROM deliveries").first())?.count,
    0,
  )
})

test("clock and crypto are substituted at composition and payloads are not logged", async () => {
  const reports: unknown[] = []
  const verifier = githubPayloadVerifier(secret, webcrypto.subtle as SubtleCrypto)
  const handler = createWebhookHandler({
    verifier,
    store: {
      retain: async (delivery) => {
        assert.equal(delivery.receivedAt, "2026-10-25T00:00:00.000Z")
        return "stored"
      },
    },
    now: () => new Date("2026-10-25T00:00:00Z"),
    report: (...args) => reports.push(args),
  })
  const payload = '{"private":"content"}'
  const result = await handler(
    new Request("https://test/github/webhook", {
      method: "POST",
      headers: signed(payload),
      body: payload,
    }),
  )
  assert.equal(result.status, 202)
  assert.equal(JSON.stringify(reports).includes("private"), false)
})
