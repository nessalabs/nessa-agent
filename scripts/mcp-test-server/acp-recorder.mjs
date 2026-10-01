#!/usr/bin/env node
/**
 * Stand between the gateway and an ACP agent, passing every byte through
 * unchanged and appending each newline-delimited frame to a log, so a live run
 * records exactly what the harness said and what it was asked.
 *
 *   node acp-recorder.mjs <log.jsonl> <command> [args...]
 *
 * Each log line is `{"direction":"to-agent"|"from-agent","frame":<json>}`;
 * a line that is not JSON is kept as `"text"`. Standard error passes through
 * unrecorded. The recorder exits with the agent's status, and ends the agent
 * when its own input closes or it is signalled.
 *
 * What is recorded is the protocol stream, which carries prompts, tool
 * arguments and results — never credentials, which the gateway hands the
 * agent in its environment. Review a recording before checking any of it in.
 */
import { spawn } from "node:child_process"
import { appendFileSync } from "node:fs"
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

function main([log, command, ...args]) {
  if (!log || !command) {
    process.stderr.write("usage: acp-recorder.mjs <log.jsonl> <command> [args...]\n")
    process.exit(2)
  }
  const child = spawn(command, args, { stdio: ["pipe", "pipe", "inherit"] })
  let inbound = ""
  let outbound = ""
  process.stdin.setEncoding("utf8")
  child.stdout.setEncoding("utf8")
  process.stdin.on("data", (chunk) => {
    inbound = lines(inbound, chunk, (line) =>
      appendFileSync(log, record("to-agent", line)),
    )
    child.stdin.write(chunk)
  })
  process.stdin.on("end", () => child.stdin.end())
  child.stdout.on("data", (chunk) => {
    outbound = lines(outbound, chunk, (line) =>
      appendFileSync(log, record("from-agent", line)),
    )
    process.stdout.write(chunk)
  })
  for (const signal of ["SIGTERM", "SIGINT", "SIGHUP"])
    process.on(signal, () => child.kill(signal))
  child.on("exit", (code, signal) => process.exit(code ?? (signal ? 1 : 0)))
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main(process.argv.slice(2))
