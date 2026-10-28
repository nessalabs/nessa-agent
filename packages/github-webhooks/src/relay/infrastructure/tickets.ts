import type { WakeTickets } from "../application/ports.js"
import { relayLimits, repositoryIdentity } from "../domain/cursor.js"
const encode = (bytes: Uint8Array) =>
  btoa(String.fromCharCode(...bytes))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "")
function decode(text: string): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(atob(text.replaceAll("-", "+").replaceAll("_", "/")), (c) =>
    c.charCodeAt(0),
  )
}
export class HmacWakeTickets implements WakeTickets {
  private readonly key: Promise<CryptoKey>
  constructor(
    secret: string,
    private readonly stage: string,
    private readonly subtle: SubtleCrypto,
    private readonly now: () => number,
  ) {
    this.key = subtle.importKey(
      "raw",
      new TextEncoder().encode(secret),
      { name: "HMAC", hash: "SHA-256" },
      false,
      ["sign", "verify"],
    )
  }
  async issue(userId: string, repositoryId: string): Promise<string> {
    const body = encode(
      new TextEncoder().encode(
        JSON.stringify({
          userId,
          repositoryId,
          stage: this.stage,
          expiresAt: this.now() + relayLimits.ticketMs,
        }),
      ),
    )
    return (
      body +
      "." +
      encode(
        new Uint8Array(
          await this.subtle.sign(
            "HMAC",
            await this.key,
            new TextEncoder().encode("nessa-relay-watch\n" + body),
          ),
        ),
      )
    )
  }
  async verify(ticket: string) {
    try {
      if (ticket.length > 1024) return null
      const parts = ticket.split(".")
      if (
        parts.length !== 2 ||
        !(await this.subtle.verify(
          "HMAC",
          await this.key,
          decode(parts[1]),
          new TextEncoder().encode("nessa-relay-watch\n" + parts[0]),
        ))
      )
        return null
      const value = JSON.parse(new TextDecoder().decode(decode(parts[0])))
      if (
        value.stage !== this.stage ||
        repositoryIdentity(value.repositoryId) === null ||
        repositoryIdentity(value.userId) === null ||
        !Number.isSafeInteger(value.expiresAt) ||
        value.expiresAt <= this.now() ||
        value.expiresAt > this.now() + relayLimits.ticketMs
      )
        return null
      return {
        userId: value.userId as string,
        repositoryId: value.repositoryId as string,
        expiresAt: value.expiresAt as number,
      }
    } catch {
      return null
    }
  }
}
