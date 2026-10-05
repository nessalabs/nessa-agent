/**
 * The scenario files the checks run. The recorded claude and codex frames
 * are not one of them: a scripted agent with no `--scenario` replays those.
 */
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))

/** A plain reply, for a check that compares the window with a text turn. */
export const TEXT_REPLY_SCENARIO = join(here, "scenarios/text-reply.json")

/**
 * Permission, a streamed reply, a tool call, a mid-turn failure, and a
 * cancel. `scripted-scenarios.mjs` drives it through the gateway and the window.
 */
export const WINDOW_SCENARIO = join(here, "scenarios/window.json")
