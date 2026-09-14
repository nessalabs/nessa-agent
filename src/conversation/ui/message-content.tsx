import * as React from "react"
import { Clipboard } from "lucide-react"
import { MessageMarkdown } from "@nessa-ui/react/message-markdown"
import { MathBlock } from "@nessa-ui/react/math-block"
import { Button } from "@nessa-ui/react/button"
import type { MessageContent } from "../model"
import { pastedTextLabel } from "./composer-content"

/** Render one Markdown document, replacing only this message's owned pasted-text slots. */
export function MessageContentView({
  content,
  onOpenPaste,
}: {
  content: MessageContent
  onOpenPaste: (text: string) => void
}) {
  const slots = React.useMemo(() => {
    const nonce = crypto.randomUUID()
    const pastes = new Map<string, string>()
    const markdown = content
      .map((part, index) => {
        if (part.type === "text") return part.text
        const url = `https://nessa-paste.invalid/${nonce}/${index}`
        pastes.set(url, part.text)
        return `[pasted text](${url})`
      })
      .join("")
    return { markdown, pastes }
  }, [content])
  const pill = (url: string, text: string) => (
    <Button
      key={url}
      variant="ghost"
      size="sm"
      className="nessa-pasted-pill inline-flex h-auto max-w-full gap-1 rounded-full px-1 py-0 align-baseline text-inherit"
      onClick={() => onOpenPaste(text)}
    >
      <Clipboard aria-hidden="true" className="size-3 shrink-0" />
      <span className="truncate">{pastedTextLabel(text)}</span>
    </Button>
  )
  return (
    <MessageMarkdown
      className="text-inherit [&_p]:whitespace-pre-wrap [&_a]:text-inherit [&_blockquote]:text-inherit [&_th]:text-foreground [&_code]:text-foreground"
      components={{
        a: ({ href, children, node: _node, ...props }) => {
          const pasted = href === undefined ? undefined : slots.pastes.get(href)
          return href !== undefined && pasted !== undefined ? (
            pill(href, pasted)
          ) : (
            <a href={href} {...props}>
              {children}
            </a>
          )
        },
        code: ({ children, node: _node, ...props }) => {
          if (
            (props.className ?? "").includes("math-inline") &&
            typeof children === "string"
          ) {
            return <MathBlock inline tex={children} />
          }
          let nodes: React.ReactNode[] = [children]
          for (const [url, text] of slots.pastes) {
            nodes = nodes.flatMap<React.ReactNode>((child) =>
              typeof child !== "string"
                ? [child]
                : child
                    .split(`[pasted text](${url})`)
                    .flatMap((value, index) =>
                      index ? [pill(url, text), value] : [value],
                    ),
            )
          }
          return <code {...props}>{nodes}</code>
        },
      }}
    >
      {slots.markdown}
    </MessageMarkdown>
  )
}
