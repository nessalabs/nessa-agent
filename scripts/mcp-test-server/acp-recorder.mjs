#!/usr/bin/env node
/**
 * Stand between the gateway and an ACP agent, passing every byte through
 * unchanged and appending each newline-delimited frame to a log, so a live run
 * records exactly what the harness said and what it was asked.
 *
 *   node acp-recorder.mjs <log.jsonl> <command> [args...]
 *
 * Each log line is `{"direction":"to-agent"|"from-agent","frame":<json>}`;
 * a line that is not JSON is kept as `"text"`, and a last line without a
 * newline is logged when its stream ends. Standard error passes through
 * unrecorded. It supervises nothing: it exits with the agent's status (128 +
 * the signal number when the agent was signalled, as a shell reports it) once
 * the agent's output has been delivered, and a signal sent to it ends it as it
 * would any process — the agent then sees its input close. Input is logged as
 * it is read and forwarded; once the agent has exited or closed its input,
 * nothing more is read, so input still buffered then may be logged without
 * having been delivered.
 *
 * What is recorded is the protocol stream, which carries prompts, tool
 * arguments and results — never credentials, which the gateway hands the
 * agent in its environment. Review a recording before checking any of it in.
 */
import { spawn } from "node:child_process"
import { appendFileSync } from "node:fs"
import { constants as osConstants } from "node:os"
import { StringDecoder } from "node:string_decoder"
import { fileURLToPath } from "node:url"

/** One log line for one frame travelling in `direction`. */
export function record(direction, line) {
  let frame
  try {
    frame = JSON.parse(line)
  } catch {
    return `${JSON.stringify({ direction, text: line })}\n`
  }
  return `${JSON.stringify({ direction, frame })}\n`
}

/** Split `chunk` onto `pending`, calling `each` with every complete line. */
export function lines(pending, chunk, each) {
  const parts = (pending + chunk).split("\n")
  const rest = parts.pop()
  for (const part of parts) if (part.trim()) each(part)
  return rest
}

/** A logger for one direction: whole lines as they complete, the rest at the end. */
function direction(log, name) {
  const decoder = new StringDecoder("utf8")
  let pending = ""
  const write = (line) => appendFileSync(log, record(name, line))
  return {
    take: (chunk) => {
      pending = lines(pending, decoder.write(chunk), write)
    },
    end: () => {
      const rest = pending + decoder.end()
      pending = ""
      if (rest.trim()) write(rest)
    },
  }
}

function main([log, command, ...args]) {
  if (!log || !command) {
    process.stderr.write("usage: acp-recorder.mjs <log.jsonl> <command> [args...]\n")
    process.exit(2)
  }
  const fail = (error) => {
    process.stderr.write(`acp-recorder: ${error?.message ?? error}\n`)
    process.exit(1)
  }
  const child = spawn(command, args, { stdio: ["pipe", "pipe", "inherit"] })
  child.on("error", fail)
  // Stop reading (and logging) input the agent can no longer receive.
  const release = () => {
    process.stdin.unpipe(child.stdin)
    process.stdin.destroy()
  }
  // An agent that stops reading its input is the agent's business, as it
  // would be without the recorder: its own status still decides the exit.
  child.stdin.on("error", (error) => {
    if (error.code !== "EPIPE" && error.code !== "ERR_STREAM_DESTROYED") fail(error)
    release()
  })
  process.stdout.on("error", fail)
  const toAgent = direction(log, "to-agent")
  const fromAgent = direction(log, "from-agent")
  // Bytes pass through untouched, with the pipes' own backpressure; the log
  // sees the same chunks and only it decodes them.
  process.stdin.on("data", (chunk) => toAgent.take(chunk))
  process.stdin.on("end", () => toAgent.end())
  process.stdin.pipe(child.stdin)
  child.stdout.on("data", (chunk) => fromAgent.take(chunk))
  child.stdout.on("end", () => fromAgent.end())
  child.stdout.pipe(process.stdout)
  // The agent's status, or 128 + the signal. Not `process.exit`, which would
  // drop output still queued for a slow reader: stop reading input and let
  // the process end once everything written has been delivered.
  child.on("exit", (code, signal) => {
    process.exitCode = code ?? 128 + (signal ? (osConstants.signals[signal] ?? 0) : 0)
    release()
  })
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main(process.argv.slice(2))
