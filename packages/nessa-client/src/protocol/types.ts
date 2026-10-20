/**
 * Wire types generated from `protocol/schemas/v1/`.
 * Regenerate: `pnpm protocol:generate`
 */
export type {
  ClientInfo,
  ClientRole,
  EventFrame,
  GatewayError,
  HealthResult,
  ReqFrame,
  ResFrame,
  Scope,
  ShortcutAction,
  ShortcutArgs,
  ShortcutBinding,
  ShortcutScope,
  ShortcutSurface,
  ShortcutsDocument,
  SurfaceInfo,
} from "../generated/protocol.js"

import type { EventFrame, ReqFrame, ResFrame } from "../generated/protocol.js"

/** Any JSON message on the WebSocket. */
export type Frame = ReqFrame | ResFrame | EventFrame

export type {
  ProductSessionReady,
  SessionAuthenticateParams,
  SessionChallenge,
} from "./product-types.js"
export { ProductEvent, ProductMethod } from "./product-types.js"

/** Server push events the client understands today. */
export type ClientEventMap = {
  "session.challenge": import("./product-types.js").SessionChallenge
  "conversation.view": import("../generated/product.js").ConversationViewed
  "conversation.listed": import("../generated/product.js").ConversationListed
  "conversation.events": import("../generated/product.js").ConversationEvents
  "conversation.subscriptionEnded": import("../generated/product.js").ConversationSubscriptionEnded
  "permission.reviewWaiting": import("../generated/product.js").PermissionReviewWaiting
}
