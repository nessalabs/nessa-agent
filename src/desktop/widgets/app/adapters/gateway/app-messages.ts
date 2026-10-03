/**
 * `McpAppConversation` over the gateway (`client.mcpApps`, #390): an app's
 * `ui/message` goes to its conversation as `mcp.sendMessage`, and its
 * `ui/update-model-context` as `mcp.updateModelContext`, each naming the
 * app's mount. Whether it may — the person's consent the first time, a turn
 * already running, the bounds — is the gateway's to decide; this says only
 * whether it was taken.
 *
 * What the app sent is already read into blocks of text
 * (`model/messages.ts`); a message is their text, a blank line between each,
 * and a context their text beside its structured content, encoded. Both are
 * held to the client's bounds (`mcpAppRequestProblem`) before anything is
 * sent: past them the app is told it was refused, and nothing is logged.
 *
 * A refusal the gateway answers with a code is `refused`. Anything else —
 * no answer, one the client did not believe, a fault around the call — is
 * not an answer: the port rejects, and the bridge logs it and tells the app
 * the request failed (`ports.ts`).
 */
import {
  mcpAppDeadlines,
  mcpAppRequestProblem,
  NessaMcpAppError,
  type McpAppsApi,
} from "@nessa/client"
import type { AppAddress, Delivered, McpAppConversation } from "../../application/ports"
import type { JsonObject } from "../../model/json-rpc"

/** What the gateway is asked of an app's conversation. */
export type AppConversationApi = Pick<McpAppsApi, "sendMessage" | "updateModelContext">

/** The text of `content`'s blocks, a blank line between each. */
function textOf(content: readonly JsonObject[]): string {
  return content
    .map((block) => (typeof block.text === "string" ? block.text : ""))
    .join("\n\n")
}

/** What a call that threw comes to: a refusal the gateway answered, or a fault. */
function refusedOr(error: unknown): Delivered {
  if (error instanceof NessaMcpAppError && error.code !== undefined) return "refused"
  throw error
}

/** The app's conversation, through the gateway's `client.mcpApps`. */
export function gatewayAppConversation(mcpApps: AppConversationApi): McpAppConversation {
  return {
    // The longest the client waits for `mcp.sendMessage`: a review, then the send.
    messageWithin: mcpAppDeadlines.sendMessageMs,

    async sendMessage(address: AppAddress, content: readonly JsonObject[]) {
      const text = textOf(content)
      if (mcpAppRequestProblem.message(text)) return "refused"
      try {
        await mcpApps.sendMessage(address.sessionId, address.app, address.server, text)
        return "done"
      } catch (error) {
        return refusedOr(error)
      }
    },

    async updateModelContext(address, context) {
      const text = context.content ? textOf(context.content) : ""
      const update = {
        ...(text ? { text } : {}),
        ...(context.structuredContent
          ? { structuredContentJson: JSON.stringify(context.structuredContent) }
          : {}),
      }
      if (mcpAppRequestProblem.context(update)) return "refused"
      try {
        await mcpApps.updateModelContext(
          address.sessionId,
          address.app,
          address.server,
          update,
        )
        return "done"
      } catch (error) {
        return refusedOr(error)
      }
    },
  }
}
