import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod } from "../generated/product.js"
import type {
  PeerEnrollParams,
  PeerFollowParams,
  PeerFollowResult,
  PeerForgetParams,
  PeerGateway,
  PeerListResult,
} from "../generated/product.js"

/**
 * This gateway's peers: enroll into one, choose which of its granted chats
 * to keep, and read what the poller last saw. The gateway decides; a refusal
 * is the RPC error it answered with.
 */
export type PeerApi = {
  /** Enroll with this gateway's own key. The code is a gateway invitation. */
  enroll(params: PeerEnrollParams): Promise<PeerGateway>
  /** Store which of one peer's granted chats this gateway replicates. */
  follow(params: PeerFollowParams): Promise<PeerFollowResult>
  /** Forget one peer and its retained cache. */
  forget(params: PeerForgetParams): Promise<PeerGateway>
  /** Every peer this gateway keeps. */
  list(): Promise<PeerListResult>
}

export function createPeerApi(session: RpcRequester): PeerApi {
  return {
    enroll: (params) =>
      session.request(ProductMethod.PeerEnroll, params).then(peerGateway),
    follow: (params) =>
      session.request(ProductMethod.PeerFollow, params).then(peerFollowResult),
    forget: (params) =>
      session.request(ProductMethod.PeerForget, params).then(peerGateway),
    list: () => session.request(ProductMethod.PeerList, {}).then(peerList),
  }
}

function peerKey(value: unknown): number[] {
  if (!Array.isArray(value) || value.some((byte) => !Number.isInteger(byte)))
    throw new TypeError("a peer answer named no key")
  return value as number[]
}

function peerGateway(value: unknown): PeerGateway {
  if (typeof value !== "object" || value === null || !("phase" in value))
    throw new TypeError("a peer answer was not a peer")
  const peer = value as PeerGateway
  peerKey(peer.peerKey)
  return peer
}

function peerFollowResult(value: unknown): PeerFollowResult {
  if (
    typeof value !== "object" ||
    value === null ||
    !("conversations" in value) ||
    !Array.isArray((value as PeerFollowResult).conversations)
  )
    throw new TypeError("a follow answer was not a stored rule")
  const result = value as PeerFollowResult
  peerKey(result.peerKey)
  return result
}

function peerList(value: unknown): PeerListResult {
  if (
    typeof value !== "object" ||
    value === null ||
    !("items" in value) ||
    !Array.isArray((value as PeerListResult).items)
  )
    throw new TypeError("a peer list was not a list of peers")
  for (const item of (value as PeerListResult).items) peerGateway(item)
  return value as PeerListResult
}
