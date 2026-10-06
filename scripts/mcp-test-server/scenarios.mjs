/**
 * The scenario files the checks run. The recorded claude and codex frames
 * are not one of them: a scripted agent with no `--scenario` replays those
 * on its first completed prompt, then answers a later prompt with text.
 */
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))

/**
 * A plain reply, for a check that compares the window with a text turn,
 * and one turn that calls `review_rows` when the prompt contains
 * `TEXT_REPLY_APP_PROMPT`.
 */
export const TEXT_REPLY_APP_PROMPT = "show the server's app"
export const TEXT_REPLY_SCENARIO = join(here, "scenarios/text-reply.json")

/**
 * Permission, a streamed reply, a tool call, a mid-turn failure, and a
 * cancel. `scripted-scenarios.mjs` drives it through the gateway and the window.
 */
export const WINDOW_SCENARIO = join(here, "scenarios/window.json")
