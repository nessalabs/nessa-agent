/** Retained gateway input references through the production desktop mapper and renderer. */
import { createRoot } from "react-dom/client"
import { transcriptFrom } from "../../../../src/desktop/workspace/adapters/gateway/gateway-views"
import { view } from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"
import { Message } from "../../../../src/desktop/workspace/ui/transcript/message"
import "@fontsource-variable/geist"
import "../../../../src/desktop/styles.css"
import "../../../../src/desktop/workspace/ui/transcript/transcript.css"

const image = { digest: "a".repeat(64), mimeType: "image/png" as const, size: 2048 }
const upload = {
  digest: `sha256:${"b".repeat(64)}`,
  mimeType: "application/pdf",
  size: 512,
  name: "meeting notes.pdf",
}
const file = { path: "/workspace/quarterly [report].csv" }
const scenarios = ["committed", "pending", "restored"].flatMap((phase) =>
  ["image", "file", "upload", "mixed"].map((kind) => {
    const attachments = ["image", "mixed"].includes(kind) ? [image] : []
    const files = ["file", "mixed"].includes(kind) ? [file] : []
    const uploads = ["upload", "mixed"].includes(kind) ? [upload] : []
    const text = kind === "mixed" ? "Inspect `this` [report], please." : ""
    const wire = view("original", {
      messages:
        phase === "pending"
          ? []
          : [
              {
                executionId: "e1",
                userText: text,
                attachments,
                files,
                uploads,
                status: "completed",
                parts: [],
              },
            ],
      pending:
        phase === "pending"
          ? [{ executionId: "e1", mode: "queued", text, attachments, files, uploads }]
          : [],
    })
    const transcript = transcriptFrom(
      phase === "restored" ? JSON.parse(JSON.stringify(wire)) : wire,
      1,
      () => 1,
      1000,
    )
    return { phase, kind, message: transcript.messages[0] }
  }),
)
const element = document.getElementById("root")
if (!element) throw new Error("Attachment fixture root missing")
createRoot(element).render(
  <main
    data-user-attachments
    data-surface="desktop"
    style={{ padding: 16, height: "100%", overflow: "auto" }}
  >
    {scenarios.map(({ phase, kind, message }) => (
      <section
        key={`${phase}-${kind}`}
        data-input-case={`${phase}-${kind}`}
        style={{ marginBottom: 16 }}
      >
        <h2>
          {phase}: {kind}
        </h2>
        <Message sessionId="original" message={message} isNew={false} />
      </section>
    ))}
  </main>,
)
