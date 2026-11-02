/**
 * A realtime voice provider, on this machine, for the stress check's `voice`
 * step: the one HTTP call the gateway makes (`crates/nessa-server/src/voice`)
 * answered as OpenAI documents it, and the call itself as a WebSocket that
 * speaks the same realtime events with no audio (the window's `scripted`
 * transport).
 *
 * ```text
 *   POST /v1/realtime/client_secrets   a 60 s secret, for the saved key only
 *   WS   /realtime?secret=…            one call per secret handed out
 * ```
 *
 * The person's side is played by `say(text)`: the provider hears them, writes
 * their words down, and decides to call `send_to_agent`, as the coordinator
 * instructions ask. `ask(name)` makes the voice call any tool. Everything the
 * window sends is kept in `received`, and each `response.create` is answered
 * with a short spoken line, so the window sees the voice speak and stop.
 *
 * `provider(file)` writes the file the gateway reads from
 * `NESSA_VOICE_REALTIME`, naming this server.
 */
import { randomBytes } from "node:crypto"
import { createServer } from "node:http"
import { writeFileSync } from "node:fs"
import { WebSocketServer } from "ws"

/** The key the step saves in Settings; any other is refused, as OpenAI would. */
export const FAKE_VOICE_KEY = `sk-stress-${randomBytes(12).toString("hex")}`
/** What the voice says each time it is asked to respond. */
export const FAKE_VOICE_LINE = "On it."

/** Starts the fake on a free loopback port. Resolves with its controls. */
export async function startFakeRealtime({ key = FAKE_VOICE_KEY } = {}) {
  const secrets = new Set()
  /** Every client-secret request's body, as the gateway sent it. */
  const minted = []
  /** Every event the window sent on the open call. */
  const received = []
  let socket = null
  let calls = 0
  let items = 0
  const json = (response, status, body) => {
    response.writeHead(status, { "content-type": "application/json" })
    response.end(JSON.stringify(body))
  }
  const server = createServer((request, response) => {
    let body = ""
    request.on("data", (chunk) => (body += chunk))
    request.on("end", () => {
      const url = new URL(request.url ?? "/", "http://fake")
      if (request.method === "POST" && url.pathname === "/v1/realtime/client_secrets") {
        if (request.headers.authorization !== `Bearer ${key}`)
          return json(response, 401, { error: { message: "Incorrect API key provided" } })
        try {
          minted.push(JSON.parse(body))
        } catch {
          return json(response, 400, { error: { message: "unreadable body" } })
        }
        const value = `ek_${randomBytes(12).toString("hex")}`
        secrets.add(value)
        return json(response, 200, {
          value,
          expires_at: Math.floor(Date.now() / 1000) + 60,
        })
      }
      json(response, 404, { error: { message: "not found" } })
    })
  })
  const sockets = new WebSocketServer({ noServer: true })
  server.on("upgrade", (request, stream, head) => {
    const url = new URL(request.url ?? "/", "http://fake")
    const secret = url.searchParams.get("secret") ?? ""
    // A secret opens one call.
    if (url.pathname !== "/realtime" || !secrets.delete(secret)) {
      stream.end("HTTP/1.1 401 Unauthorized\r\n\r\n")
      return
    }
    sockets.handleUpgrade(request, stream, head, (open) => {
      calls += 1
      socket = open
      open.on("message", (data) => {
        let event
        try {
          event = JSON.parse(String(data))
        } catch {
          return
        }
        received.push(event)
        if (event.type === "response.create") speak(FAKE_VOICE_LINE)
      })
      open.on("close", () => {
        if (socket === open) socket = null
      })
    })
  })
  const emit = (event) => socket?.send(JSON.stringify(event))
  const speak = (line) => {
    const response = `resp_${(items += 1)}`
    emit({ type: "response.created", response: { id: response } })
    emit({
      type: "response.output_audio_transcript.delta",
      response_id: response,
      delta: line,
    })
    emit({ type: "response.done", response: { id: response, status: "completed" } })
  }
  /**
   * The voice decides to call `name`, in a response of its own. As OpenAI
   * does, the call is an item of the response's `response.done`; a response
   * the person talked over ends `cancelled`, and its call is not to be made.
   */
  const ask = (name, status = "completed") => {
    const response = `resp_${(items += 1)}`
    emit({ type: "response.created", response: { id: response } })
    emit({
      type: "response.done",
      response: {
        id: response,
        status,
        output: [
          {
            type: "function_call",
            name,
            call_id: `call_${(items += 1)}`,
            arguments: "{}",
          },
        ],
      },
    })
  }
  /** The person says `text`, heard in two halves, and the voice sends it. */
  const say = (text, tool = "send_to_agent") => {
    const item = `item_${(items += 1)}`
    const half = Math.ceil(text.length / 2)
    emit({ type: "input_audio_buffer.speech_started", item_id: item })
    emit({ type: "input_audio_buffer.committed", item_id: item })
    emit({
      type: "conversation.item.input_audio_transcription.delta",
      item_id: item,
      delta: text.slice(0, half),
    })
    emit({
      type: "conversation.item.input_audio_transcription.delta",
      item_id: item,
      delta: text.slice(half),
    })
    emit({
      type: "conversation.item.input_audio_transcription.completed",
      item_id: item,
      transcript: text,
    })
    if (tool) ask(tool)
  }
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
  const { port } = server.address()
  const base = `http://127.0.0.1:${port}`
  return {
    key,
    minted,
    received,
    say,
    ask,
    /** Whether a call is open now. */
    open: () => socket !== null,
    /** How many calls were opened. */
    calls: () => calls,
    /** The text of every system message the window gave the voice. */
    told: () =>
      received
        .filter((event) => event.type === "conversation.item.create")
        .map((event) => event.item)
        .filter((item) => item?.type === "message")
        .map((item) => item.content?.[0]?.text ?? ""),
    /** Every tool output the window answered, in order. */
    outputs: () =>
      received
        .filter((event) => event.type === "conversation.item.create")
        .map((event) => event.item)
        .filter((item) => item?.type === "function_call_output")
        .map((item) => item.output),
    provider(file) {
      writeFileSync(
        file,
        JSON.stringify({
          apiBase: base,
          transport: "scripted",
          callUrl: `ws://127.0.0.1:${port}/realtime`,
        }),
      )
      return file
    },
    async close() {
      for (const client of sockets.clients) client.terminate()
      sockets.close()
      await new Promise((resolve) => server.close(() => resolve()))
    },
  }
}
