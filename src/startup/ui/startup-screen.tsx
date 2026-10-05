import face from "../../host/startup-face.html?raw"
import { startupLine } from "../../host/startup-refusals"
import "../../host/startup-screen.css"

function escapeText(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;")
}

/**
 * The startup screen, from `startup-face.html`: the same mark, line, code,
 * and icon actions the load fallback paints before the module runs.
 */
export function StartupScreen({
  code,
  onRestart,
  onQuit,
}: {
  code: string
  onRestart: () => void
  onQuit: () => void
}) {
  const html = face
    .replaceAll("{{LINE}}", escapeText(startupLine()))
    .replaceAll("{{CODE}}", escapeText(code))
  return (
    <main
      data-nessa-startup-screen=""
      data-nessa-startup-overlay=""
      role="alert"
      dangerouslySetInnerHTML={{ __html: html }}
      onClick={(event) => {
        const target = event.target
        if (!(target instanceof Element)) return
        const action = target.closest("[data-nessa-startup-action]")
        const name = action?.getAttribute("data-nessa-startup-action")
        if (name === "restart") onRestart()
        else if (name === "quit") onQuit()
        else if (name === "copy") {
          const text = action?.textContent?.trim() ?? ""
          void navigator.clipboard?.writeText(text)
        }
      }}
    />
  )
}
