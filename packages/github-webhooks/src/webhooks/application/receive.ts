import type { Delivery } from "../domain/delivery.js"

export type StoreResult = "stored" | "duplicate" | "conflict" | "full"
export interface DeliveryStore {
  retain(delivery: Delivery): Promise<StoreResult>
}

export type ReceiveResult = StoreResult | "unavailable"

export async function receive(
  delivery: Delivery,
  store: DeliveryStore,
): Promise<ReceiveResult> {
  try {
    return await store.retain(delivery)
  } catch {
    // A lost answer is not acceptance. Redelivery uses the same delivery ID.
    return "unavailable"
  }
}
