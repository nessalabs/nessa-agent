import type { SqlDatabase } from "../../webhooks/infrastructure/d1.js"
import type { RelayReadBudget } from "../application/ports.js"

/** A single row per verified GitHub user, bounded even under identity churn.
 * Admission and counters share one atomic statement; failed reads still count. */
export class D1RelayReadBudget implements RelayReadBudget {
  constructor(
    private readonly database: SqlDatabase,
    private readonly now: () => number,
  ) {}
  async admit(userId: string): Promise<boolean> {
    const day = Math.floor(this.now() / 86_400_000)
    const minute = Math.floor(this.now() / 60_000)
    const row = await this.database
      .prepare(
        `
      INSERT INTO relay_user_budget(user_id,day,day_reads,minute,minute_reads)
      SELECT ?, ?, 1, ?, 1 WHERE
        EXISTS(SELECT 1 FROM relay_user_budget WHERE user_id=?) OR
        (SELECT COUNT(*) FROM relay_user_budget)<4096
      ON CONFLICT(user_id) DO UPDATE SET
        day=excluded.day, day_reads=CASE WHEN day=excluded.day THEN day_reads+1 ELSE 1 END,
        minute=excluded.minute, minute_reads=CASE WHEN minute=excluded.minute THEN minute_reads+1 ELSE 1 END
      WHERE excluded.day>=day AND excluded.minute>=minute AND (day<>excluded.day OR day_reads<6000) AND (minute<>excluded.minute OR minute_reads<120)
      RETURNING user_id`,
      )
      .bind(userId, day, minute, userId)
      .first()
    return row !== null
  }
  async expire(): Promise<void> {
    await this.database
      .prepare(
        `DELETE FROM relay_user_budget WHERE user_id IN (
      SELECT user_id FROM relay_user_budget WHERE day < ? LIMIT 64
    ) RETURNING user_id`,
      )
      .bind(Math.floor(this.now() / 86_400_000) - 1)
      .all()
  }
}
