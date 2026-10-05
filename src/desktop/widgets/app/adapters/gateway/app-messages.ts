/**
 * `McpAppConversation` over the gateway (`client.mcpApps`, #390): an app's
 * `ui/message` goes to its conversation as `mcp.sendMessage`, and its
 * `ui/update-model-context` as `mcp.updateModelContext`, each naming the
 * app's mount. Whether it may — the person's consent to each message, a turn
 * already running, the bounds — is the gateway's to decide; this says what it
 * answered (the desktop's table on #390, rows D1–D16).
 *
 * What the app sent is already read into blocks of text
 * (`model/messages.ts`), and `contentText` makes them the one text sent on:
 * a message's, or a context's beside its structured content, encoded. A
 * context with neither part — `{}`, no blocks, or only empty ones — is sent
 * with neither, which clears what the mount gave (D12, the gateway's C7).
 * Both are held to the client's bounds (`mcpAppRequestProblem`), its owner,
 * before anything is sent: past them the answer is `invalid`, in the client's
 * words, and nothing is logged (D3, D13).
 *
 * What the gateway answers with a code is read through the one table the
 * server port reads (`outcomes` in `mcp-app-server.ts`, by `answerFor`);
 * anything else — no answer, one the client did not believe, a fault around
 * the call — is `failed`, and logged there (D7).
 *
 * One mount's context updates go one after another, in the order the app
 * gave them, each once the one before has answered, whatever it answered:
 * two sent at once could reach the gateway the other way round, and the one
 * recorded last stands (D15, the gateway's C8). Another mount's are not held
 * back. A message is not queued: every one waits on its own review.
 */
import { mcpAppDeadlines, mcpAppRequestProblem, type McpAppsApi } from "@nessa/client"
import type { ConversationAnswer, McpAppConversation } from "../../application/ports"
import { contentText } from "../../model/messages"
import { answerFor } from "./mcp-app-server"

/** What the gateway is asked of an app's conversation. */
export type AppConversationApi = Pick<McpAppsApi, "sendMessage" | "updateModelContext">

/** Taken: a message is the conversation's turn, a context is recorded (C17). */
const taken: ConversationAnswer = { kind: "ok", result: {} }

/** The app's conversation, through the gateway's `client.mcpApps`. */
export function gatewayAppConversation(mcpApps: AppConversationApi): McpAppConversation {
  // Each mount's last update, sent or waiting to be, by its `instanceId`;
  // settled, whatever it answered, before the next is sent.
  const updating = new Map<string, Promise<unknown>>()
  const inTurn = (
    mount: string,
    update: () => Promise<ConversationAnswer>,
  ): Promise<ConversationAnswer> => {
    const sent = (updating.get(mount) ?? Promise.resolve()).then(update)
    const settled = sent.then(
      () => undefined,
      () => undefined,
    )
    updating.set(mount, settled)
    void settled.then(() => {
      if (updating.get(mount) === settled) updating.delete(mount)
    })
    return sent
  }

  return {
    // The client waits a call's deadline for both: a review, then the send (K-2).
    within: mcpAppDeadlines.callToolMs,

    async sendMessage(address, content) {
      const text = contentText(content)
      const problem = mcpAppRequestProblem.message(text)
      if (problem) return { kind: "invalid", reason: problem }
      try {
        await mcpApps.sendMessage(address.sessionId, address.app, address.server, text)
        return taken
      } catch (error) {
        return answerFor(error, "conversation")
      }
    },

    async updateModelContext(address, context) {
      const text = contentText(context.content ?? [])
      // Absent parts are left out: with neither, the update is a clear.
      const update = {
        ...(text ? { text } : {}),
        ...(context.structuredContent
          ? { structuredContentJson: JSON.stringify(context.structuredContent) }
          : {}),
      }
      const problem = mcpAppRequestProblem.context(update)
      if (problem) return { kind: "invalid", reason: problem }
      return inTurn(address.app.instanceId, async () => {
        try {
          await mcpApps.updateModelContext(
            address.sessionId,
            address.app,
            address.server,
            update,
          )
          return taken
        } catch (error) {
          return answerFor(error, "conversation")
        }
      })
    },
  }
}
