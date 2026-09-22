import { ToolPayload } from "./tool-payload"
import { ToolCall, ToolCallTrigger, ToolCallContent } from "@nessa-ui/react/tool-call"
import { MessageMarkdown } from "@nessa-ui/react/message-markdown"
import type { AgentToolView, WorkStep } from "./agent-transcript-view"

/**
 * A turn's working, in the order it happened, so a rationale still reads
 * beside the call it explains. Loaded only when the sheet opens; the shared
 * tool renderer includes diff support.
 */
export default function WorkSteps({ work }: { work: WorkStep[] }) {
  return (
    <div className="flex flex-col gap-4 select-text">
      {work.map((step) => (
        <div key={step.key}>
          {/* What the agent said on the way is prose it wrote, not a thought. */}
          {step.text !== undefined && (
            <MessageMarkdown streaming={false}>{step.text}</MessageMarkdown>
          )}
          {step.thought !== undefined && (
            <p className="text-sm whitespace-pre-wrap text-muted-foreground">
              {step.thought}
            </p>
          )}
          {step.tool && <ToolRow tool={step.tool} />}
        </div>
      ))}
    </div>
  )
}

function ToolRow({ tool }: { tool: AgentToolView }) {
  return (
    <ToolCall
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
  )
}
