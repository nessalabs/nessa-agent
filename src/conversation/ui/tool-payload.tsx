import { useMemo } from "react"
import { JsonTree } from "@nessa-ui/react/json-tree"

const textClass =
  "m-0 min-w-0 max-w-full whitespace-pre-wrap [overflow-wrap:anywhere] font-mono text-xs"

/** JSON stays structured; stream fields are decoded once for readable terminal output. */
export function ToolPayload({ text }: { text: string }) {
  const parsed = useMemo(() => {
    try {
      return { valid: true as const, value: JSON.parse(text) as unknown }
    } catch {
      return { valid: false as const }
    }
  }, [text])
  if (!parsed.valid) return <pre className={textClass}>{text}</pre>
  const value = parsed.value
  const streams: [string, string][] = []
  let metadata = value
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    metadata = Object.fromEntries(
      Object.entries(value).filter(([key, field]) => {
        if ((key === "stdout" || key === "stderr") && typeof field === "string") {
          streams.push([key, field])
          return false
        }
        return true
      }),
    )
  }
  return (
    <div className="flex min-w-0 flex-col gap-2">
      {streams.map(([name, content]) => (
        <section key={name} aria-label={name}>
          <p className="m-0 text-xs text-muted-foreground">{name}</p>
          <pre className={textClass}>{content || "(empty)"}</pre>
        </section>
      ))}
      <JsonTree key={text} value={metadata} collapsible />
    </div>
  )
}
