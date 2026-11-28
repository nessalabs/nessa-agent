// This app's own server. Nessa starts it when the app opens and stops it
// when the app closes. It talks to Nessa through `@nessa/client`, signed in
// with this app's own credential: it sees only the chats it started.
import { createServer } from "node:http"
import { createHash, randomUUID } from "node:crypto"
import { NessaClient } from "@nessa/client"

const env = process.env

// It listens at once, so its window opens even while Nessa is away, and
// signs in to Nessa beside that, retrying until it can.
let nessa
let unavailable = "Signing in to Nessa…"
async function signIn() {
  for (;;) {
    try {
      nessa = await NessaClient.connect({
        stage: env.NESSA_STAGE,
        role: "surface",
        surface: { kind: "desktop", instance: env.NESSA_APP_ID },
        client: { id: env.NESSA_APP_ID, version: "1.0.0", platform: "node" },
        profile: "product",
        auth: { credential: env.NESSA_APP_CREDENTIAL },
      })
      return
    } catch (error) {
      unavailable = `Can't reach Nessa: ${error.message}`
      await new Promise((resolve) => setTimeout(resolve, 5_000))
    }
  }
}
void signIn()

const routes = {
  // The chats this app started, newest first.
  "GET /chats": async () => {
    const { conversations } = await nessa.conversation.list({})
    return conversations.map((row) => ({ id: row.conversationId, title: row.title }))
  },
  // Start a chat with `text` as its first message.
  "POST /chats": async ({ text }) => {
    if (typeof text !== "string" || !text.trim()) throw new Error("say something")
    const id = randomUUID()
    await nessa.conversation.create({ conversationId: id })
    await nessa.conversation.submit(nessa.conversation.prepare(id, text))
    return { id }
  },
}

const MAX_BODY = 64 * 1024

async function body(request) {
  let text = ""
  for await (const chunk of request) {
    text += chunk
    if (text.length > MAX_BODY) throw new Error("too large")
  }
  return text ? JSON.parse(text) : {}
}

// Nessa asks this before it opens the window: the answer proves this is
// the server it started, without the token ever being sent.
const ready = createHash("sha256").update(env.NESSA_APP_TOKEN).digest("hex")

createServer(async (request, response) => {
  if (request.method === "GET" && request.url === "/.nessa-ready")
    return response.writeHead(200, { "content-type": "text/plain" }).end(ready)
  // Only this app's window holds the token, so no other page can use
  // the app's access through this server.
  response.setHeader("access-control-allow-origin", "*")
  response.setHeader("access-control-allow-headers", "content-type, x-nessa-app-token")
  if (request.method === "OPTIONS") return response.writeHead(204).end()
  if (request.headers["x-nessa-app-token"] !== env.NESSA_APP_TOKEN)
    return response.writeHead(401).end()
  const route = routes[`${request.method} ${new URL(request.url, "http://app").pathname}`]
  if (!route) return response.writeHead(404).end()
  if (!nessa)
    return response
      .writeHead(503, { "content-type": "application/json" })
      .end(JSON.stringify({ error: unavailable }))
  try {
    const answer = await route(await body(request))
    response
      .writeHead(200, { "content-type": "application/json" })
      .end(JSON.stringify(answer))
  } catch (error) {
    response
      .writeHead(400, { "content-type": "application/json" })
      .end(JSON.stringify({ error: error.code ?? error.message }))
  }
}).listen(Number(env.NESSA_APP_PORT), env.NESSA_APP_HOST)
