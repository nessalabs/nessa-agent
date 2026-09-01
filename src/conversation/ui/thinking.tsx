import { ChatTypingIndicator } from "@nessa-ui/react/chat-bubbles"

/**
 * The typing pill. Motion hosts use the design-system indicator (WAAPI pulse).
 * Layout hosts get the same pill without the translate animation — CSS cannot
 * cancel a WAAPI `translate`, and that is what ghosts on a transparent window.
 */
export function Thinking({ motion }: { motion: boolean }) {
  if (motion) return <ChatTypingIndicator label="Nessa is typing" />
  return (
    <div
      role="status"
      aria-label="Nessa is typing"
      data-slot="chat-typing-indicator"
      className="flex items-center gap-1 self-start rounded-[1.125rem] bg-accent px-3.5 py-3"
    >
      {[0, 1, 2].map((dot) => (
        <span
          key={dot}
          data-slot="chat-typing-dot"
          className="size-2 rounded-full bg-muted-foreground opacity-35"
        />
      ))}
    </div>
  )
}
