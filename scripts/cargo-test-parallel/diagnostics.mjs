const OUTPUT_LIMIT = 16_384

// Workflow command data escapes keep test output inside one annotation, including
// literal workflow commands. diagnostics.test.mjs exercises percent/CR/LF input.
function annotation(message) {
  const data = message
    .replaceAll("%", "%25")
    .replaceAll("\r", "%0D")
    .replaceAll("\n", "%0A")
  return `::error title=Cargo test process failed::${data}\n`
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
      write(
        annotation(
          `${command} ${JSON.stringify(args)} ${outcome}\n` +
            `${truncated ? "Output truncated; last" : "Captured output; at most"} ${OUTPUT_LIMIT} characters:\n${tail}`,
        ),
      )
    }
    let code
    try {
      code = await run(command, args, {
        ...io,
        onStdout: (chunk) => capture(chunk, io.onStdout),
        onStderr: (chunk) => capture(chunk, io.onStderr),
      })
    } catch (error) {
      report(`failed: ${error.message}`)
      throw error
    }
    if (code !== 0) report(`exited with ${code}`)
    return code
  }
}
