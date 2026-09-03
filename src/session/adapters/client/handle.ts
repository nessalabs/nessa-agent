import type { NessaClient } from "@nessa/client"

/**
 * Process-local handle to the live wire client.
 *
 * Kept outside Redux on purpose — `NessaClient` is not serializable. Chat
 * adapters should call {@link getSessionClient} rather than opening a second
 * socket.
 */
let client: NessaClient | null = null

export function getSessionClient(): NessaClient | null {
  return client
}

export function setSessionClient(next: NessaClient | null): void {
  client = next
}
