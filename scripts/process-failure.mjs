import { spawn } from "node:child_process"
import { pathToFileURL } from "node:url"
import { createHash } from "node:crypto"

const OUTPUT_LIMIT = 16_384
const PREVIEW_LIMIT = 1024
const PART_LIMIT = 3300
const hash = (text) => createHash("sha256").update(text).digest("hex")

// Do not split a valid surrogate pair at an annotation or preview boundary.
function prefix(text, limit) {
  let end = Math.min(text.length, limit)
  const left = text.charCodeAt(end - 1)
  const right = text.charCodeAt(end)
  if (left >= 0xd800 && left <= 0xdbff && right >= 0xdc00 && right <= 0xdfff) end--
  return text.slice(0, end)
}

function preview(value) {
  const text = String(value)
  return text.length <= PREVIEW_LIMIT
    ? text
    : `${prefix(text, PREVIEW_LIMIT)}\n[preview truncated; ${text.length} UTF-16 units; sha256=${hash(text)}]`
}

function parts(text) {
  const result = []
  for (let end = text.length; end > 0;) {
    const start = prefix(text, Math.max(0, end - PART_LIMIT)).length
    result.unshift(text.slice(start, end))
    end = start
  }
  return result
}

// Workflow command data escapes keep test output inside one annotation, including
// literal workflow commands. process-failure.test.mjs exercises percent/CR/LF input.
function annotation(message) {
  const data = message
    .replaceAll("%", "%25")
    .replaceAll("\r", "%0D")
    .replaceAll("\n", "%0A")
  return `::error title=Check process failed::${data}\n`
}

/** Decorate execution with bounded diagnostics; the child owns its exit/result.
 * Commands, arguments, callbacks and rejected errors pass through unchanged.
 * Only a failed process emits an annotation. Unit and real-Cargo tests enforce
 * that diagnostics do not decide whether a test passed or which tests run.
 */
export function withFailureAnnotations(run, write) {
  return async (command, args, io) => {
    let tail = ""
    let truncated = false
    function capture(chunk, forward) {
      truncated ||= tail.length + chunk.length > OUTPUT_LIMIT
      tail = (tail + chunk.slice(-OUTPUT_LIMIT)).slice(-OUTPUT_LIMIT)
      forward?.(chunk)
    }
    function report(outcome) {
      try {
        const text =
          `${preview(`${command} ${JSON.stringify(args)}`)} ${outcome()}\n` +
          `${truncated ? "Output truncated; last" : "Captured output; at most"} ${OUTPUT_LIMIT} characters:\n${tail}`
        const digest = hash(text)
        const chunks = parts(text)
        // Emit the last report part first so the retained tail has priority.
        for (let index = chunks.length - 1; index >= 0; index--)
          write(
            annotation(
              `Retained report ${digest} part ${index + 1}/${chunks.length}\n${chunks[index]}`,
            ),
          )
      } catch {
        // Reporting must not replace the failed child's result or rejection.
        // process-failure.test.mjs exercises throwing writers and rejection getters.
      }
    }
    let code
    try {
      code = await run(command, args, {
        ...io,
        onStdout: (chunk) => capture(chunk, io.onStdout),
        onStderr: (chunk) => capture(chunk, io.onStderr),
      })
    } catch (error) {
      report(() => `failed: ${preview(error?.message ?? error)}`)
      throw error
    }
    if (code !== 0) report(() => `exited with ${preview(code)}`)
    return code
  }
}

export function spawnProcess(command, args, io) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: io.cwd,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    })
    child.stdout.setEncoding("utf8")
    child.stderr.setEncoding("utf8")
    child.stdout.on("data", (chunk) => io.onStdout?.(chunk))
    child.stderr.on("data", (chunk) => io.onStderr?.(chunk))
    child.on("error", reject)
    child.on("close", (code) => resolve(code ?? 1))
  })
}

/** The one owner of CI-only annotation activation for process callers. */
export function runProcess(command, args, io) {
  const run =
    process.env.GITHUB_ACTIONS === "true"
      ? withFailureAnnotations(spawnProcess, (line) => process.stderr.write(line))
      : spawnProcess
  return run(command, args, io)
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  try {
    const [command, ...args] = process.argv.slice(2)
    if (!command)
      throw new Error("usage: process-failure.mjs <executable> [arguments...]")
    process.exitCode = await runProcess(command, args, {
      cwd: process.cwd(),
      onStdout: (chunk) => process.stdout.write(chunk),
      onStderr: (chunk) => process.stderr.write(chunk),
    })
  } catch (error) {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  }
}
