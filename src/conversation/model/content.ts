/** Ordered message content; pasted payloads remain literal and independently viewable. */
export type MessagePart =
  { type: "text"; text: string } | { type: "pasted-text"; id: string; text: string }

export type MessageContent = MessagePart[]

/** Text sent to the agent, retaining the exact order and whitespace of pasted payloads. */
export function contentText(content: MessageContent): string {
  return content.map((part) => part.text).join("")
}

/** Creates an ordinary Markdown draft. */
export function textContent(text: string): MessageContent {
  return text ? [{ type: "text", text }] : []
}
