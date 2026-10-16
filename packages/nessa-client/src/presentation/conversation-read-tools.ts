import type { ConversationListResult, ConversationView } from "../generated/product.js"
import { bounds } from "../generated/product.js"
import { conversationIdPattern } from "../protocol/conversation-validate.js"
import type { ConversationApi } from "./conversation-api.js"

const tools = Object.freeze([
  Object.freeze({
    name: "nessa_conversations_list",
    description: `List the authenticated caller's conversations, most recently updated first, at most ${bounds.maxListedConversations}. Set archived to true for archived conversations only; false or omitted lists the rest. complete: false means the list is incomplete.`,
    inputSchema: Object.freeze({
      type: "object",
      properties: Object.freeze({
        archived: Object.freeze({ type: "boolean" }),
      }),
      required: Object.freeze([]),
      additionalProperties: false,
    }),
  }),
  Object.freeze({
    name: "nessa_conversation_read",
    description:
      "Read a conversation's bounded current replacement view, including lifecycle, messages, pending input, tools and permission choices. Inspect truncated and transcriptState for incomplete history. This does not open a provider or send a message.",
    inputSchema: Object.freeze({
      type: "object",
      properties: Object.freeze({
        conversationId: Object.freeze({
          type: "string",
          pattern: conversationIdPattern.source,
        }),
      }),
      required: Object.freeze(["conversationId"]),
      additionalProperties: false,
    }),
  }),
] as const)

/** One framework-neutral tool descriptor with a JSON Schema input contract. */
export type ConversationReadToolDescriptor = {
  /** Name accepted by the dispatcher. */
  readonly name: "nessa_conversations_list" | "nessa_conversation_read"
  /** Meaning and completeness limits of the returned evidence. */
  readonly description: string
  /** JSON Schema describing the exact accepted argument object. */
  readonly inputSchema: {
    /** Arguments are an object. */
    readonly type: "object"
    /** Accepted own fields and their primitive types. */
    readonly properties: Readonly<
      Record<string, { readonly type: "boolean" | "string"; readonly pattern?: string }>
    >
    /** Fields the caller must provide. */
    readonly required: readonly string[]
    /** Fields outside properties are refused. */
    readonly additionalProperties: false
  }
}

/** The host supplies its existing authenticated conversation API; no connection is opened here. */
export type ConversationReadToolsOptions = {
  readonly conversation: Pick<ConversationApi, "list" | "read">
}

/** Optional model-callable reads over the host's authenticated client. */
export type ConversationReadTools = {
  /** Frozen descriptors to translate into the host's model tool format. */
  readonly tools: readonly ConversationReadToolDescriptor[]
  /**
   * Dispatch one read. Unknown names, non-data objects, extra fields and malformed
   * arguments reject with TypeError before calling the injected API.
   * Results and failures from that API pass through without caching or retries.
   */
  readonly call: (
    name: string,
    args: unknown,
  ) => Promise<ConversationListResult | ConversationView>
}

/** Accept own data fields only, as a parsed JSON object would supply. */
function argumentsObject(
  args: unknown,
  fields: readonly string[],
): Record<string, unknown> {
  if (args === null || typeof args !== "object" || Array.isArray(args))
    throw new TypeError("Tool arguments must be an object")
  const prototype: unknown = Object.getPrototypeOf(args)
  if (prototype !== Object.prototype && prototype !== null)
    throw new TypeError("Tool arguments must be a plain object")
  for (const key of Reflect.ownKeys(args)) {
    if (typeof key !== "string" || !fields.includes(key))
      throw new TypeError("Unknown tool argument")
    const descriptor = Object.getOwnPropertyDescriptor(args, key)
    if (!descriptor || !Object.hasOwn(descriptor, "value"))
      throw new TypeError("Tool arguments must contain data fields")
  }
  return args as Record<string, unknown>
}

/**
 * Expose list/read as tools using an injected client API. The trusted host owns
 * endpoint selection, authentication and connection lifecycle. The gateway's
 * existing list/read methods require conversation.write; this adapter grants
 * no authority and does not enable read-only credentials.
 */
export function createConversationReadTools({
  conversation,
}: ConversationReadToolsOptions): ConversationReadTools {
  return {
    tools,
    async call(name, args) {
      switch (name) {
        case "nessa_conversations_list": {
          const input = argumentsObject(args, ["archived"])
          if (!Object.hasOwn(input, "archived")) return conversation.list({})
          const archived = input.archived
          if (typeof archived !== "boolean")
            throw new TypeError("archived must be a boolean")
          return conversation.list({ archived })
        }
        case "nessa_conversation_read": {
          const input = argumentsObject(args, ["conversationId"])
          const conversationId = Object.hasOwn(input, "conversationId")
            ? input.conversationId
            : undefined
          if (
            typeof conversationId !== "string" ||
            !conversationIdPattern.test(conversationId)
          )
            throw new TypeError("conversationId must be a canonical UUID")
          return conversation.read(conversationId)
        }
        default:
          throw new TypeError("Unknown conversation read tool")
      }
    },
  }
}
