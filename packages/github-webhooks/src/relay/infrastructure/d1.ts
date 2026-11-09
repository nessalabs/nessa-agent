import type {
  RelayInbox,
  RelayPage,
  RelayEvent,
  RelayReadAudit,
} from "../application/ports.js"
import type { SqlDatabase } from "../../webhooks/infrastructure/d1.js"
import { relayLimits } from "../domain/cursor.js"

export class D1RelayInbox implements RelayInbox {
  constructor(private readonly database: SqlDatabase) {}
  async page(repositoryId: string, after: number): Promise<RelayPage> {
    // One read snapshot includes an empty-page watermark. Separate rows avoid
    // D1's row-size limit when several maximum-size payloads share a page.
    const rows = await this.database
      .prepare(
        `WITH candidates AS (
      SELECT e.sequence, d.delivery_id AS deliveryId, d.event, d.payload, d.bytes,
        d.received_at AS receivedAt, e.repository_id AS repositoryId
      FROM relay_events e JOIN deliveries d USING(delivery_id)
      WHERE e.repository_id = ? AND e.sequence > ? ORDER BY e.sequence LIMIT ?
    ), ranked AS (SELECT *, SUM(bytes) OVER (ORDER BY sequence) AS totalBytes,
      ROW_NUMBER() OVER (ORDER BY sequence) AS position FROM candidates),
    selected AS (SELECT * FROM ranked WHERE position <= ? AND (totalBytes <= ? OR position = 1))
    SELECT watermark.expired, EXISTS(SELECT 1 FROM candidates WHERE sequence >
      (SELECT MAX(sequence) FROM selected)) AS more, selected.* FROM
      (SELECT COALESCE((SELECT through_sequence FROM relay_expiry WHERE repository_id = ?),0) AS expired) watermark
      LEFT JOIN selected ON 1=1 ORDER BY selected.sequence`,
      )
      .bind(
        repositoryId,
        after,
        relayLimits.pageSize + 1,
        relayLimits.pageSize,
        relayLimits.pageBytes,
        repositoryId,
      )
      .all<{
        expired: number
        more: number
        sequence: number | null
        deliveryId: string
        event: string
        payload: string
        receivedAt: string
        repositoryId: string
      }>()
    if (rows.results.length === 0) throw new Error("missing relay page")
    const events: RelayEvent[] = []
    for (const row of rows.results) {
      if (row.sequence !== null)
        events.push({
          sequence: row.sequence,
          deliveryId: row.deliveryId,
          event: row.event,
          payload: row.payload,
          receivedAt: row.receivedAt,
          repositoryId: row.repositoryId,
        })
    }
    return {
      expiredThrough: rows.results[0].expired,
      hasMore: rows.results[0].more === 1,
      events,
    }
  }
  async expire(before: string): Promise<void> {
    await this.database
      .prepare(
        `DELETE FROM deliveries WHERE delivery_id IN
      (SELECT delivery_id FROM deliveries WHERE received_at < ? ORDER BY received_at LIMIT ?) RETURNING delivery_id`,
      )
      .bind(before, relayLimits.sweepRows)
      .all()
    // Audit is metadata only and bounded separately from payload retention.
    await this.database
      .prepare(
        `DELETE FROM relay_cleanup_audit WHERE cleanup_id <=
      (SELECT MAX(cleanup_id) - ? FROM relay_cleanup_audit) RETURNING cleanup_id`,
      )
      .bind(relayLimits.auditRows)
      .all()
  }
}

export class D1RelayReadAudit implements RelayReadAudit {
  constructor(
    private readonly database: SqlDatabase,
    private readonly now: () => Date,
  ) {}
  async record(input: {
    userId: string
    repositoryId: string
    allowed: boolean
    rules: readonly string[]
    after: number
  }) {
    const receipt = await this.database
      .prepare(
        "INSERT INTO relay_read_audit(user_id,repository_id,allowed,rules,after_sequence,observed_at) VALUES(?,?,?,?,?,?) RETURNING audit_id",
      )
      .bind(
        input.userId,
        input.repositoryId,
        input.allowed ? 1 : 0,
        JSON.stringify(input.rules),
        input.after,
        this.now().toISOString(),
      )
      .first<{ audit_id: number }>()
    if (receipt === null) throw new Error("audit_unconfirmed")
  }
}
