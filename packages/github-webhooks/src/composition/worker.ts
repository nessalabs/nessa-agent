import { D1DeliveryStore, type SqlDatabase } from "../webhooks/infrastructure/d1.js"
import { createWebhookHandler } from "../webhooks/infrastructure/http.js"
import { githubPayloadVerifier } from "../webhooks/infrastructure/signature.js"

interface Environment {
  DB: SqlDatabase
  GITHUB_WEBHOOK_SECRET?: string
}

export default {
  async fetch(request: Request, environment: Environment): Promise<Response> {
    const secret = environment.GITHUB_WEBHOOK_SECRET
    if (!environment.DB || typeof secret !== "string" || secret.length < 32) {
      return Response.json({ result: "not_configured" }, { status: 503 })
    }
    const handler = createWebhookHandler({
      store: new D1DeliveryStore(environment.DB),
      verifier: githubPayloadVerifier(secret, crypto.subtle),
      now: () => new Date(),
      report: (deliveryId, event, result) => {
        console.log(JSON.stringify({ deliveryId, event, result }))
      },
    })
    return handler(request)
  },
}
