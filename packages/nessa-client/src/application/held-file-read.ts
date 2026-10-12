import { NessaRpcError } from "./rpc-error.js"
import type { ConversationErrorCode } from "../generated/product.js"

/**
 * The most bytes `attachments.fetch` asks `artifact.read` for at once: the
 * maximum of `ArtifactReadParams.maxBytes` in the published schema. Not a
 * generated bound, so it is written here once, and
 * `attachment-api.test.ts` ("asks for the schema's largest range") reads the
 * schema and fails when the two differ.
 */
export const HELD_FILE_RANGE_BYTES = 32_768

/**
 * How long `attachments.fetch` waits before asking for the same range again
 * after the gateway answered `temporarily_unavailable` (its read slots were
 * taken): one wait per retry, so at most this many retries of any one range,
 * after which the fetch fails as `temporarily_unavailable`. A range that is
 * answered starts the count again for the next one.
 */
export const HELD_FILE_BUSY_RETRY_DELAYS_MS: readonly number[] = Object.freeze([
  250, 500, 1000,
])

/**
 * Why a held file was not handed back by `attachments.fetch`.
 *
 * The gateway's refusals, in the words of what they mean for the file:
 * - `not_held`: the conversation does not keep exactly this file now (digest,
 *   media type and size together) — never uploaded or published into it,
 *   expired, or released when it closed (`attachment_not_found`).
 * - `conversation_not_found`, `conversation_deleted`: as the gateway names them.
 * - `temporarily_unavailable`: its read slots stayed taken through every retry
 *   (`HELD_FILE_BUSY_RETRY_DELAYS_MS`). Fetching again later may work.
 * - `unavailable`: it holds the file but could not read it
 *   (`attachment_unavailable`).
 * - `invalid_request`, `forbidden`, `conversations_not_configured`: as the
 *   gateway names them; asking again changes nothing.
 *
 * And this client's own: `integrity` (the assembled bytes are not the ones
 * named — another size or SHA-256), `aborted` (the caller's signal),
 * `unreachable` (no answer: the connection failed or the request timed out),
 * `unexpected_response` (an answer this client does not believe, including a
 * refusal code it has not been taught — a code is never passed through as if
 * known; `cause` keeps it).
 */
export type HeldFileFailureCode =
  | "not_held"
  | "conversation_not_found"
  | "conversation_deleted"
  | "temporarily_unavailable"
  | "unavailable"
  | "invalid_request"
  | "forbidden"
  | "conversations_not_configured"
  | "integrity"
  | "aborted"
  | "unreachable"
  | "unexpected_response"

/**
 * The gateway's refusal codes for `artifact.read`, as this client names them.
 * Keyed by the generated codes rather than spelled again beside them, so a
 * code the protocol renames stops this compiling instead of quietly answering
 * `unexpected_response`. `forbidden` is the socket's access refusal, which is
 * no conversation code.
 */
const refusals: {
  readonly [
    K in
      | Extract<
          ConversationErrorCode,
          | "attachment_not_found"
          | "conversation_not_found"
          | "conversation_deleted"
          | "temporarily_unavailable"
          | "attachment_unavailable"
          | "invalid_request"
          | "conversations_not_configured"
        >
      | "forbidden"
  ]: HeldFileFailureCode
} = Object.freeze({
  attachment_not_found: "not_held",
  conversation_not_found: "conversation_not_found",
  conversation_deleted: "conversation_deleted",
  temporarily_unavailable: "temporarily_unavailable",
  attachment_unavailable: "unavailable",
  invalid_request: "invalid_request",
  conversations_not_configured: "conversations_not_configured",
  forbidden: "forbidden",
})

/**
 * What a rejected `artifact.read` means: the gateway's refusal by its code,
 * read from the RPC error and nothing else, or `unreachable` when nothing
 * answered. A code this client has not been taught is `unexpected_response`.
 */
export function heldFileRefusal(cause: unknown): HeldFileFailureCode {
  if (!(cause instanceof NessaRpcError)) return "unreachable"
  // Read for what it owns: an arbitrary wire string such as "constructor" is
  // a member of every object, and is no code this client knows.
  return Object.hasOwn(refusals, cause.code)
    ? refusals[cause.code as keyof typeof refusals]
    : "unexpected_response"
}

/**
 * A held file that was not handed back, with a {@link HeldFileFailureCode} to
 * branch on. Nothing unverified is ever returned in its place. Nothing is
 * retried for you but a busy range (`HELD_FILE_BUSY_RETRY_DELAYS_MS`).
 */
export class NessaHeldFileError extends Error {
  constructor(
    /** Which refusal or fault this was. */
    readonly code: HeldFileFailureCode,
    cause?: unknown,
  ) {
    super(`Held file was not fetched (${code})`, { cause })
    this.name = "NessaHeldFileError"
  }
}
