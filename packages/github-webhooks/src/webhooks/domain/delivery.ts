import contract from "../../../../../protocol/defaults/github-relay-contract.json"
export const inboxLimits = Object.freeze({
  bodyBytes: contract.payloadBytes,
  deliveries: contract.storedEvents,
  storedBytes: contract.storedBytes,
})

export type DeliveryIdentity = Readonly<{ id: string; event: string }>

// Header syntax is bounded here; subscribed event selection belongs to GitHub.
export function deliveryIdentity(
  id: string | null,
  event: string | null,
): DeliveryIdentity | null {
  if (
    id === null ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id) ||
    event === null ||
    !/^[a-z][a-z0-9_]{0,63}$/.test(event)
  ) {
    return null
  }
  return Object.freeze({ id: id.toLowerCase(), event })
}

export type Delivery = Readonly<{
  identity: DeliveryIdentity
  payload: string
  payloadSha256: string
  receivedAt: string
  bytes: number
}>
