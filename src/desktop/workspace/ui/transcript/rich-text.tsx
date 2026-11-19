import { Fragment, useEffect, useRef, useState } from "react"
import { reducedMotion } from "../../../adapters/motion-preference"
import { useMessageStreamText } from "@nessa-ui/react/message"
import { ReplyMarkdown } from "@nessa-ui/react/reply-markdown"
import { inlineRuns } from "../../model/transcript"
import { ChatPill, useKnownChat } from "./chat-pill"

/**
 * Inline code an agent wrote. The id of a chat this window has is that chat,
 * as a pill (`ChatPill`); anything else is code, as the renderer draws it.
 * Fenced code never reaches here: `ReplyMarkdown` draws it as a code block.
 */
function ProseCode({ children, ...rest }: React.ComponentProps<"code">) {
  const text = typeof children === "string" ? children.trim() : ""
  const known = useKnownChat(text)
  if (known) return <ChatPill sessionId={text} />
  return <code {...rest}>{children}</code>
}

/** A chat id written as plain text: that chat's pill when the window has it, else the id. */
function ProseChatId({ id }: { id: string }) {
  return useKnownChat(id) ? <ChatPill sessionId={id} /> : <>{id}</>
}

const proseComponents = { code: ProseCode }

const uuid = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi

/**
 * Each whole id in a run of the reply's prose — `ReplyMarkdown` never hands
 * over text inside code or a link — set apart as `ProseChatId`, which draws
 * that chat's pill when the window has it, and the same text when not.
 */
function chatIds(text: string): React.ReactNode {
  const out: React.ReactNode[] = []
  let at = 0
  for (const match of text.matchAll(uuid)) {
    const index = match.index ?? 0
    if (index > at) out.push(text.slice(at, index))
    out.push(<ProseChatId key={index} id={match[0]} />)
    at = index + match[0].length
  }
  if (out.length === 0) return text
  if (at < text.length) out.push(text.slice(at))
  return out
}

/**
 * A line the window wrote itself — the person's words in their bubble, an
 * item of a sample list — with its `code` and **strong** runs drawn as such.
 * Not for what an agent wrote: that is Markdown (`AgentProse`).
 */
export function RichText({ text }: { text: string }) {
  return (
    <>
      {inlineRuns(text).map((run, index) =>
        run.kind === "code" ? (
          <code key={index}>{run.text}</code>
        ) : run.kind === "strong" ? (
          <strong key={index}>{run.text}</strong>
        ) : (
          <Fragment key={index}>{run.text}</Fragment>
        ),
      )}
    </>
  )
}

/**
 * What an agent wrote, as Markdown: paragraphs, line breaks, headings, lists,
 * links, inline code and fenced code — drawn by the design system's
 * `ReplyMarkdown`, the same renderer the panel draws a reply with
 * (`src/conversation/ui/transcript.tsx`). It parses with Words, which keeps
 * the blocks it has finished, so each streamed chunk parses only the reply's
 * tail. Its text keeps the transcript's own size, leading and colour; only
 * its block structure comes from the component. A link is a plain
 * `<a href>`, which the host hands to the person's browser rather than
 * navigating the window (`src-tauri/src/links.rs`); `ReplyMarkdown` draws no
 * raw HTML and no unsafe address, so the agent cannot author a `target`, a
 * `download` or a script.
 */
export function AgentProse({
  text,
  streaming = false,
  fresh = false,
}: {
  text: string
  /** Its turn is still writing it (`TurnMessage.working`). */
  streaming?: boolean
  /** It has only now appeared in this view (`Message`'s `isNew`). */
  fresh?: boolean
}) {
  // While it is written, the kit's pacing releases bursty chunks as one
  // steady reveal and `ReplyMarkdown` fades each new word in, so words
  // arrive in place rather than in jumps — the same reveal the panel's
  // replies have. Text already there when it mounts (a saved reply) shows at
  // once, and so does a reply already under way when it is opened (another
  // pane, a peek): only words this view has not shown yet are revealed. A
  // reply is drawn once it has words, so one that has only now appeared
  // starts from nothing and its first chunk arrives like every other.
  const [begun, setBegun] = useState(!(streaming && fresh))
  useEffect(() => setBegun(true), [])
  const paced = useMessageStreamText(begun ? text : "")
  // Revealing until every word has finished fading in, not merely appeared:
  // `done` is a frame behind a chunk that lands with the turn's end, and
  // switching off mid-fade would set the last words down at once. The
  // window's own reduced motion (not only the system's) sets them down.
  // A reply never seen being written is set down as it is.
  const streamed = useRef(streaming)
  if (streaming) streamed.current = true
  const revealing =
    !reducedMotion() &&
    (streaming || (streamed.current && paced.staticLength < text.length))
  return (
    <ReplyMarkdown
      className="workspace-prose text-inherit text-[length:inherit] leading-[inherit] tracking-[inherit]"
      streaming={revealing}
      components={proseComponents}
      renderText={chatIds}
    >
      {revealing ? paced.text : text}
    </ReplyMarkdown>
  )
}
