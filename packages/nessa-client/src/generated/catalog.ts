/* eslint-disable */
/**
 * Generated from protocol/manifest.json — do not edit by hand.
 *
 * **Source of truth:** `protocol/manifest.json` (method/event names).
 * Payload shapes → see `./protocol.ts` (from `protocol/schemas/v1/`).
 * Regenerate: `pnpm protocol:generate`
 *
 * To add a method: edit manifest + schemas, run `pnpm protocol:generate`,
 * wire the handler — do not invent string literals in client/server code.
 */

export const Method = {
  Connect: "connect",
  ConversationEcho: "conversation.echo",
  ServerHealth: "server.health",
  ServerPing: "server.ping",
} as const

export type MethodName = (typeof Method)[keyof typeof Method]

export const Event = {
  ConnectChallenge: "connect.challenge",
} as const

export type EventName = (typeof Event)[keyof typeof Event]
