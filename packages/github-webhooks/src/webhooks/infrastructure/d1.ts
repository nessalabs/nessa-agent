import type { DeliveryStore, StoreResult } from "../application/receive.js"
import { inboxLimits, type Delivery } from "../domain/delivery.js"

interface Statement {
  bind(...values: (string | number)[]): Statement
  first<T>(): Promise<T | null>
}

export interface SqlDatabase {
  prepare(sql: string): Statement
}

export class D1DeliveryStore implements DeliveryStore {
  constructor(private readonly database: SqlDatabase) {}

  async retain(delivery: Delivery): Promise<StoreResult> {
    // One SQLite statement owns capacity, deduplication and the durable receipt.
    const inserted = await this.database
      .prepare(
        `INSERT INTO deliveries
          (delivery_id, event, payload_sha256, payload, received_at, bytes)
         SELECT ?, ?, ?, ?, ?, ? FROM inbox_usage
         WHERE singleton = 1 AND deliveries < ? AND bytes + ? <= ?
         ON CONFLICT(delivery_id) DO NOTHING
         RETURNING delivery_id`,
      )
      .bind(
        delivery.identity.id,
        delivery.identity.event,
        delivery.payloadSha256,
        delivery.payload,
        delivery.receivedAt,
        delivery.bytes,
        inboxLimits.deliveries,
        delivery.bytes,
        inboxLimits.storedBytes,
      )
      .first<{ delivery_id: string }>()
    if (inserted !== null) return "stored"

    const existing = await this.database
      .prepare("SELECT event, payload_sha256 FROM deliveries WHERE delivery_id = ?")
      .bind(delivery.identity.id)
      .first<{ event: string; payload_sha256: string }>()
    if (existing === null) return "full"
    return existing.event === delivery.identity.event &&
      existing.payload_sha256 === delivery.payloadSha256
      ? "duplicate"
      : "conflict"
  }
}
