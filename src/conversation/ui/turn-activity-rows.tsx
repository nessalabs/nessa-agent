import { ToolPayload } from "./tool-payload"
import { ToolCall, ToolCallTrigger, ToolCallContent } from "@nessa-ui/react/tool-call"
import type { AgentTurnActivityItem } from "./agent-transcript-view"

/** Loaded only when the user opens a turn's internal activity. */
export default function TurnActivityRows({
  items,
}: {
  items: readonly AgentTurnActivityItem[]
}) {
  return (
    <div className="flex flex-col gap-4 select-text">
      {items.map((item) =>
        item.kind === "thought" ? (
          <section key={item.key}>
            <p className="text-xs text-muted-foreground">Thought</p>
            <p className="whitespace-pre-wrap text-sm">{item.text}</p>
          </section>
        ) : (
          <ToolCall
            key={item.key}
            status={
              item.tool.status === "failed"
                ? "error"
                : ["running", "pending"].includes(item.tool.status)
                  ? "running"
                  : "complete"
            }
          >
            <ToolCallTrigger meta={item.tool.status}>{item.tool.title}</ToolCallTrigger>
            <ToolCallContent>
              {item.tool.input && (
                <>
                  <p className="text-xs text-muted-foreground">Input</p>
                  <div
                    className="max-h-60 min-w-0 overflow-auto"
                    role="region"
                    aria-label="Tool input"
                    tabIndex={0}
                  >
                    <ToolPayload text={item.tool.input} />
                  </div>
                </>
              )}
              <p className="text-xs text-muted-foreground">Output</p>
              <div
                className="max-h-80 min-w-0 overflow-auto"
                role="region"
                aria-label="Tool output"
                tabIndex={0}
              >
                <ToolPayload
                  text={item.tool.details || "No additional details provided."}
                />
              </div>
            </ToolCallContent>
          </ToolCall>
        ),
      )}
    </div>
  )
}
