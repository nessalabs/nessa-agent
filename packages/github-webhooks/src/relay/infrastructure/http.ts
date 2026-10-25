import { readRelay } from "../application/read.js"
import type {
  GitHubIdentitySource,
  RelayInbox,
  RelayPolicyEvaluator,
  WakeTickets,
  RelayReadAudit,
} from "../application/ports.js"
import { relayCursor, repositoryIdentity } from "../domain/cursor.js"
import type { WakeRooms } from "./wake-room.js"
const reply = (status: number, body: unknown) =>
  Response.json(body, { status, headers: { "Cache-Control": "no-store" } })
export function createRelayHandler(dependencies: {
  stage: string
  identities: GitHubIdentitySource
  inbox: RelayInbox
  policy: RelayPolicyEvaluator
  tickets: WakeTickets
  rooms: WakeRooms
  audit: RelayReadAudit
}) {
  return async (request: Request): Promise<Response> => {
    const url = new URL(request.url)
    if (request.method !== "GET") return reply(405, { kind: "method_not_allowed" })
    if (request.headers.get("x-nessa-stage") !== dependencies.stage)
      return reply(409, { kind: "stage_mismatch" })
    if (url.pathname === "/relay/v1/watch") {
      const ticket = await dependencies.tickets.verify(
        request.headers.get("x-nessa-watch-ticket") ?? "",
      )
      if (ticket === null) return reply(401, { kind: "invalid_ticket" })
      return dependencies.rooms
        .getByName(`${dependencies.stage}:${ticket.repositoryId}`)
        .fetch(
          new Request("https://room/watch", {
            headers: {
              Upgrade: request.headers.get("Upgrade") ?? "",
              "x-relay-expires": String(ticket.expiresAt),
            },
          }),
        )
    }
    if (url.pathname !== "/relay/v1/events") return reply(404, { kind: "not_found" })
    const repositoryId = repositoryIdentity(url.searchParams.get("repository"))
    const expectedUser = repositoryIdentity(request.headers.get("x-github-user-id"))
    const after = relayCursor(url.searchParams.get("after"))
    if (repositoryId === null || after === null || expectedUser === null)
      return reply(400, { kind: "invalid_request" })
    const authorization = request.headers.get("Authorization") ?? ""
    if (!/^Bearer [A-Za-z0-9_]{10,255}$/.test(authorization))
      return reply(401, { kind: "unauthenticated" })
    try {
      const result = await readRelay(
        { token: authorization.slice(7), repositoryId, after, expectedUser },
        dependencies,
      )
      const statuses = {
        page: 200,
        gap: 410,
        forbidden: 403,
        identity_changed: 409,
        unauthenticated: 401,
        unavailable: 503,
      } as const
      return reply(statuses[result.kind], { ...result, stage: dependencies.stage })
    } catch {
      return reply(503, { kind: "unavailable" })
    }
  }
}
