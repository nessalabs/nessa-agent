import type { LocalStrip } from "../local-strip"

/** No-op until a remote gateway owns chat turns. */
export function sendDraft(strip: LocalStrip, _conversationId?: string): LocalStrip {
  return strip
}
