import * as React from "react"
import { Button } from "@nessa-ui/react/button"

/**
 * The technical account of a startup problem, folded away (ADR 221). It is for
 * whoever helps the person, so it can be copied whole; the person reads the
 * plain sentence above it and never has to open this.
 */
export function StartupDetails({ details }: { details: string | undefined }) {
  const [copied, setCopied] = React.useState(false)
  if (!details) return null
  return (
    <details className="nessa-text-2 text-muted-foreground">
      <summary className="cursor-pointer select-none">Details</summary>
      <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-muted/60 p-2 font-mono nessa-text-1">
        {details}
      </pre>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="mt-2 rounded-full"
        onClick={() => {
          void navigator.clipboard?.writeText(details).then(
            () => setCopied(true),
            () => setCopied(false),
          )
        }}
      >
        {copied ? "Copied" : "Copy details"}
      </Button>
    </details>
  )
}
