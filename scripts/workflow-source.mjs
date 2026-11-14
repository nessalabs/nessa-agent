/** Read workflow or action YAML as LF text for structural checks. */
import { readFileSync } from "node:fs"

export function readWorkflowSource(path) {
  return readFileSync(path, "utf8").replaceAll("\r\n", "\n")
}
