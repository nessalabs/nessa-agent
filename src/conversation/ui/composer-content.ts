import type { ChatComposerContent } from "@nessa-ui/react/chat-composer-editor"
import { contentText, type MessageContent } from "../model"

export const pastedTextLabel = (text: string) => `Pasted text (${text.length} chars)`

/** Translate UI chip DTOs into application-owned, serializable message parts. */
export function fromEditor(content: ChatComposerContent): MessageContent {
  return content.parts.map((part) =>
    part.type === "text"
      ? part
      : {
          type: "pasted-text",
          id: part.chip.id,
          text: part.chip.textValue,
        },
  )
}

/** Restore a draft with the original pasted payloads, not their display labels. */
export function toEditor(content: MessageContent): ChatComposerContent {
  // Only what the editor can hold. File tiles live beside it, in the draft.
  const editable = content.filter(
    (part) => part.type === "text" || part.type === "pasted-text",
  )
  return {
    text: contentText(content),
    parts: editable.map((part) =>
      part.type === "text"
        ? part
        : {
            type: "chip",
            chip: {
              id: part.id,
              kind: "pasted-text",
              label: pastedTextLabel(part.text),
              textValue: part.text,
            },
          },
    ),
  }
}
