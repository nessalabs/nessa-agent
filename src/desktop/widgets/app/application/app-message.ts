/**
 * The one text an app's `ui/message` blocks are sent as (#390, D-A, D3):
 * `contentText` joins them (`model/messages.ts`), and the client's bounds
 * (`mcpAppRequestProblem.message`), their owner, judge the result. Every
 * conversation an app's message reaches asks this before taking it — the
 * gateway's (`adapters/gateway/app-messages.ts`) and the sample's
 * (`fixture/fixture-plugin.ts`) — so neither takes a message the other would
 * refuse.
 *
 * It is here, not in `model/`, because the bounds are the client's: a model
 * imports no client SDK (`scripts/check-architecture.mjs`).
 */
import { mcpAppRequestProblem } from "@nessa/client"
import type { JsonObject } from "../model/json-rpc"
import { contentText } from "../model/messages"

/** The text an app's message blocks are sent as, or the client's words for why they cannot be. */
export function appMessageText(
  content: readonly JsonObject[],
):
  | { readonly kind: "text"; readonly text: string }
  | { readonly kind: "invalid"; readonly reason: string } {
  const text = contentText(content)
  const problem = mcpAppRequestProblem.message(text)
  return problem ? { kind: "invalid", reason: problem } : { kind: "text", text }
}
