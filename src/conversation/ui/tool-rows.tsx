import { ToolPayload } from "./tool-payload"
import { ToolCall, ToolCallTrigger, ToolCallContent } from "@nessa-ui/react/tool-call"
import type { AgentToolView } from "./agent-transcript-view"

/** Loaded only when the user opens tool details; the shared renderer includes diff support. */
export default function ToolRows({ tools }: { tools: AgentToolView[] }) {
  return (
    <div className="flex flex-col gap-4 select-text">
      {tools.map((tool) => (
        <ToolCall
          key={tool.callId}
          status={
            tool.status === "failed"
              ? "error"
              : ["running", "pending"].includes(tool.status)
                ? "running"
                : "complete"
          }
        >
          <ToolCallTrigger meta={tool.status}>{tool.title}</ToolCallTrigger>
          <ToolCallContent>
            {tool.input && (
              <>
                <p className="text-xs text-muted-foreground">Input</p>
                <div
                  className="max-h-60 min-w-0 overflow-auto"
                  role="region"
                  aria-label="Tool input"
                  tabIndex={0}
                >
                  <ToolPayload text={tool.input} />
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
              <ToolPayload text={tool.details || "No additional details provided."} />
            </div>
          </ToolCallContent>
        </ToolCall>
      ))}
    </div>
  )
}
