import { D1RelayReadBudget } from "../relay/infrastructure/budget.js"
import stages from "../../../../protocol/defaults/github-relays.json"
import { D1DeliveryStore, type SqlDatabase } from "../webhooks/infrastructure/d1.js"
import { createWebhookHandler } from "../webhooks/infrastructure/http.js"
import { githubPayloadVerifier } from "../webhooks/infrastructure/signature.js"
import { D1RelayInbox, D1RelayReadAudit } from "../relay/infrastructure/d1.js"
import { GitHubIdentityAdapter } from "../relay/infrastructure/github.js"
import { CedarRelayPolicyEvaluator } from "../relay/infrastructure/cedar/evaluator.js"
import { createRelayHandler } from "../relay/infrastructure/http.js"
import { HmacWakeTickets } from "../relay/infrastructure/tickets.js"
import { relayLimits } from "../relay/domain/cursor.js"
import type { WakeRooms } from "../relay/infrastructure/wake-room.js"
import { WakeRoom, type RoomState } from "../relay/infrastructure/wake-room.js"
export class RepositoryWakeRoom extends WakeRoom {
  constructor(state: RoomState) {
    super(state, Date.now)
  }
}

interface Environment {
  INGRESS_LIMIT: { limit(input: { key: string }): Promise<{ success: boolean }> }
  EDGE_LIMIT: { limit(input: { key: string }): Promise<{ success: boolean }> }
  DB: SqlDatabase
  GITHUB_WEBHOOK_SECRET?: string
  RELAY_TICKET_SECRET?: string
  RELAY_STAGE?: string
  WAKE_ROOMS?: WakeRooms
}
const policy = new CedarRelayPolicyEvaluator()
function relay(environment: Environment) {
  const stage = environment.RELAY_STAGE
  if (
    typeof stage !== "string" ||
    !environment.RELAY_TICKET_SECRET ||
    environment.RELAY_TICKET_SECRET.length < 32 ||
    !environment.WAKE_ROOMS ||
    !["dev", "alpha", "ci", "prod"].includes(stage)
  )
    return null
  const target = Object.hasOwn(stages, stage)
    ? stages[stage as keyof typeof stages]
    : null
  if (!target) return null
  return createRelayHandler({
    stage,
    budget: new D1RelayReadBudget(environment.DB, Date.now),
    audit: new D1RelayReadAudit(environment.DB, () => new Date()),
    identities: new GitHubIdentityAdapter((...args) => fetch(...args), target.appId),
    policy,
    inbox: new D1RelayInbox(environment.DB),
    tickets: new HmacWakeTickets(
      environment.RELAY_TICKET_SECRET,
      stage,
      crypto.subtle,
      Date.now,
    ),
    rooms: environment.WAKE_ROOMS,
  })
}
export default {
  async fetch(request: Request, environment: Environment): Promise<Response> {
    const secret = environment.GITHUB_WEBHOOK_SECRET
    if (!environment.DB || typeof secret !== "string" || secret.length < 32)
      return Response.json({ result: "not_configured" }, { status: 503 })
    const path = new URL(request.url).pathname
    if (path.startsWith("/relay/")) {
      // Edge limits are per Cloudflare location. No IP or credential is persisted.
      if (!environment.EDGE_LIMIT)
        return Response.json({ kind: "not_configured" }, { status: 503 })
      const ip = request.headers.get("cf-connecting-ip") ?? "unknown"
      const digest = new Uint8Array(
        await crypto.subtle.digest(
          "SHA-256",
          new TextEncoder().encode(
            request.headers.get("Authorization") ??
              request.headers.get("x-nessa-watch-ticket") ??
              "",
          ),
        ),
      )
      const key = Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join(
        "",
      )
      if (
        !(await environment.EDGE_LIMIT.limit({ key: `ip:${ip}` })).success ||
        !(await environment.EDGE_LIMIT.limit({ key: `credential:${key}` })).success
      )
        return Response.json(
          { kind: "rate_limited" },
          { status: 429, headers: { "Retry-After": "60", "Cache-Control": "no-store" } },
        )

      const handler = relay(environment)
      return handler
        ? handler(request)
        : Response.json({ kind: "not_configured" }, { status: 503 })
    }
    if (path === "/github/webhook") {
      if (!environment.INGRESS_LIMIT)
        return Response.json({ kind: "not_configured" }, { status: 503 })
      if (
        !(
          await environment.INGRESS_LIMIT.limit({
            key: request.headers.get("cf-connecting-ip") ?? "unknown",
          })
        ).success
      )
        return Response.json(
          { kind: "rate_limited" },
          { status: 429, headers: { "Retry-After": "60", "Cache-Control": "no-store" } },
        )
    }
    const handler = createWebhookHandler({
      store: new D1DeliveryStore(environment.DB),
      verifier: githubPayloadVerifier(secret, crypto.subtle),
      now: () => new Date(),
      report: (deliveryId, event, result) =>
        console.log(JSON.stringify({ deliveryId, event, result })),
    })
    const response = await handler(request)
    if (
      path === "/github/webhook" &&
      response.ok &&
      environment.WAKE_ROOMS &&
      environment.RELAY_STAGE
    ) {
      try {
        const id = request.headers.get("x-github-delivery") ?? ""
        const route = await environment.DB.prepare(
          "SELECT repository_id FROM relay_events WHERE delivery_id = ?",
        )
          .bind(id)
          .first<{ repository_id: string }>()
        if (route)
          await environment.WAKE_ROOMS.getByName(
            `${environment.RELAY_STAGE}:${route.repository_id}`,
          ).fetch(new Request("https://room/wake", { method: "POST" }))
      } catch {
        console.warn("relay_wake_unconfirmed")
      }
    }
    return response
  },
  async scheduled(_controller: unknown, environment: Environment) {
    await new D1RelayReadBudget(environment.DB, Date.now).expire()
    await new D1RelayInbox(environment.DB).expire(
      new Date(Date.now() - relayLimits.retentionMs).toISOString(),
    )
  },
}
