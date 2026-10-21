/**
 * Moves a stopped gateway's agent runtime the way a desktop update does
 * (`conversation-stress.mjs`, step `relocate`): every release stages the
 * runtime under a new directory and re-registers the service, which reads
 * the person's login-shell search path again. So the runtime's command is
 * launched from a new directory, and the agent gets a new search path and a
 * new scratch directory. None of that is which context a conversation keeps,
 * so a saved conversation must open again after it (#1071).
 *
 * The gateway must be stopped: this writes the config it reads at start.
 */
import { mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs"
import { basename, isAbsolute, join } from "node:path"

/**
 * Points every configured runtime's command at a link to it in a new
 * directory, and resolves with the environment the moved gateway starts
 * with: a search path led by that directory and a new `TMPDIR`. `count`
 * names the move, so each one lands somewhere new; `path` is the gateway's
 * config file and `directory` its data directory.
 */
export function relocateRuntime(path, directory, count) {
  const config = JSON.parse(readFileSync(path, "utf8"))
  const moved = join(directory, `runtime-${count}`)
  const scratch = join(directory, `tmp-${count}`)
  mkdirSync(moved)
  mkdirSync(scratch)
  for (const runtime of Object.values(config.agents.runtimes)) {
    if (!isAbsolute(runtime.command))
      throw new Error(`the runtime command ${runtime.command} is not a path to move`)
    const link = join(moved, basename(runtime.command))
    symlinkSync(runtime.command, link)
    runtime.command = link
  }
  writeFileSync(path, JSON.stringify(config), { mode: 0o600 })
  return {
    NESSA_AGENT_PATH: `${moved}:${process.env.PATH}`,
    TMPDIR: scratch,
  }
}
