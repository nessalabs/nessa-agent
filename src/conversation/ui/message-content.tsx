import * as React from "react"
import { Clipboard } from "lucide-react"
import { MessageMarkdown } from "@nessa-ui/react/message-markdown"
import { Button } from "@nessa-ui/react/button"
import type { MessageContent } from "../model"
import { pastedTextLabel } from "./composer-content"

type MarkdownNode = {
  type: string
  value?: string
  children?: MarkdownNode[]
  position?: { start: { offset?: number }; end: { offset?: number } }
  data?: { hName?: string; hProperties?: Record<string, string> }
  [key: string]: unknown
}

/** Parse the whole document, then replace owned slots before Markdown becomes HTML. */
function pastedDocument(content: MessageContent) {
  const prefix = `nessapaste${crypto.randomUUID().replaceAll("-", "")}x`
  const pastes = new Map<string, string>()
  const source = content
    .map((part, index) => {
      if (part.type === "text") return part.text
      const token = `${prefix}${index}z`
      pastes.set(token, part.text)
      return token
    })
    .join("")
  const pattern = new RegExp(`${prefix}\\d+z`, "g")
  const split = (value: string): MarkdownNode[] => {
    const nodes: MarkdownNode[] = []
    let offset = 0
    for (const match of value.matchAll(pattern)) {
      if (!pastes.has(match[0])) continue
      if (match.index > offset)
        nodes.push({ type: "text", value: value.slice(offset, match.index) })
      nodes.push({
        type: "pastedSlot",
        data: { hName: "span", hProperties: { "data-paste-slot": match[0] } },
        children: [],
      })
      offset = match.index + match[0].length
    }
    if (offset < value.length) nodes.push({ type: "text", value: value.slice(offset) })
    return nodes
  }
  const plugin = () => (tree: MarkdownNode) => {
    const visit = (node: MarkdownNode): MarkdownNode[] => {
      if (node.type === "text" && node.value?.includes(prefix)) return split(node.value)
      // URL destinations, HTML, code, and math can swallow inline nodes. Render
      // only these containing constructs literally, keeping the pill reachable.
      if (
        Object.entries(node).some(
          ([key, value]) =>
            key !== "type" && typeof value === "string" && value.includes(prefix),
        )
      ) {
        const start = node.position?.start.offset
        const end = node.position?.end.offset
        const raw =
          start !== undefined && end !== undefined
            ? source.slice(start, end)
            : (node.value ?? "")
        return [
          {
            type: "pastedLiteral",
            data: {
              hName: [
                "html",
                "inlineCode",
                "inlineMath",
                "link",
                "image",
                "linkReference",
                "imageReference",
              ].includes(node.type)
                ? "span"
                : "div",
            },
            children: split(raw),
          },
        ]
      }
      if (node.children) node.children = node.children.flatMap(visit)
      const containsSlot = (child: MarkdownNode): boolean =>
        child.type === "pastedSlot" || Boolean(child.children?.some(containsSlot))
      if (
        ["link", "linkReference"].includes(node.type) &&
        node.children?.some(containsSlot)
      ) {
        return [{ type: "pastedGroup", data: { hName: "span" }, children: node.children }]
      }
      return [node]
    }
    tree.children = tree.children?.flatMap(visit)
  }
  return { source, pastes, plugin }
}

/** Preserve surrounding Markdown while pasted payloads remain independently viewable. */
export function MessageContentView({
  content,
  onOpenPaste,
}: {
  content: MessageContent
  onOpenPaste: (text: string) => void
}) {
  const document = React.useMemo(() => pastedDocument(content), [content])
  return (
    <MessageMarkdown
      className="text-inherit [&_p]:whitespace-pre-wrap [&_a]:text-inherit [&_blockquote]:text-inherit [&_th]:text-foreground [&_code]:text-foreground"
      remarkPlugins={[document.plugin]}
      components={{
        span: ({ node, children, ...props }) => {
          const token = node?.properties["data-paste-slot"]
          const text = typeof token === "string" ? document.pastes.get(token) : undefined
          if (text === undefined) return <span {...props}>{children}</span>
          return (
            <Button
              variant="ghost"
              size="sm"
              className="nessa-pasted-pill inline-flex h-auto max-w-full gap-1 rounded-full px-1 py-0 align-baseline text-inherit"
              onClick={() => onOpenPaste(text)}
            >
              <Clipboard aria-hidden="true" className="size-3 shrink-0" />
              <span className="truncate">{pastedTextLabel(text)}</span>
            </Button>
          )
        },
        // A table containing slots has synthetic source; do not expose its
        // Markdown-source copy control. Ordinary tables keep the default.
        ...(document.pastes.size
          ? {
              table: ({
                node: _node,
                ...props
              }: React.ComponentProps<"table"> & { node?: unknown }) => (
                <table {...props} />
              ),
            }
          : {}),
      }}
    >
      {document.source}
    </MessageMarkdown>
  )
}
