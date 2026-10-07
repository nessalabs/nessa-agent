/**
 * The gateway stress `alpha-perf.mjs` seeds and then judges.
 *
 * A scripted gateway, `paneLimits.maxPanes` conversations, and one of them
 * holding several copies of a long user message, each answered with the
 * scenario's text. `conversation.list` has to
 * be complete before a page is opened: an incomplete list is the catalogue
 * walk, and this measurement does not time that walk. The client refuses a
 * message over its own byte bound; this text stays on the ASCII sentence the
 * seeded builder stores, cut to `characters`.
 */
import { randomUUID } from "node:crypto"
import { setTimeout as sleep } from "node:timers/promises"

import { turnEnded } from "../../../../scripts/mcp-test-server/evidence.mjs"
import { turnFor } from "../../../../scripts/mcp-test-server/scripted-scenario.mjs"
import { CannotRun, log } from "./cli.mjs"

export const gatewayStress = {
  turns: 4,
  characters: 4_000,
  marker: "ALPHA-STRESS-LINE ",
  sentence: "The transcript line stays on the page. ",
}

const conversationIdPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** The long user message: the marker, then the sentence, cut to `gatewayStress.characters`. */
export function longUserText() {
  const { marker, sentence, characters } = gatewayStress
  const body = sentence.repeat(Math.ceil(characters / sentence.length))
  return (marker + body).slice(0, characters)
}

/**
 * The text the stress scenario answers every prompt with. The scenario file
 * is the only copy; a turn that does not say text is not a stress to measure.
 */
export function scriptedReplyText(scenario) {
  const turn = turnFor(scenario, longUserText())
  const step = turn?.steps?.find((each) => each.do === "text")
  const text = Array.isArray(step?.chunks) ? step.chunks.join("") : ""
  if (text === "") throw new CannotRun("the stress scenario does not answer with text")
  return text
}

/** Visible agent prose on one stored turn: text parts, in order. Thoughts are not drawn. */
export function storedReplyText(message) {
  const parts = Array.isArray(message?.parts) ? message.parts : []
  return parts
    .filter((part) => part?.kind === "text")
    .map((part) => part.text)
    .join("")
}

/**
 * A session-list row for one gateway conversation. The id is the only thing
 * that may appear in the selector, so a value that is not one cannot change
 * what the selector matches.
 */
export function sessionRowSelector(id) {
  if (typeof id !== "string" || !conversationIdPattern.test(id))
    throw new Error("a session row selector needs a conversation id")
  return `[data-session-row="${id}"]`
}

/**
 * What the gateway has to have stored before a page is worth opening.
 * An incomplete list, a missing id, a clipped view, or a long message the
 * read does not contain is a reason not to measure.
 */
export function seedHeld(seed) {
  const failures = []
  if (seed.listComplete !== true)
    failures.push(
      "conversation.list was not complete, so this stress is not a list-only load",
    )
  if (seed.listedCount !== seed.ids.length)
    failures.push(
      `conversation.list named ${seed.listedCount}, seeded ${seed.ids.length}`,
    )
  for (const id of seed.ids ?? []) {
    if (!seed.listedIds?.includes(id)) failures.push(`conversation.list omitted ${id}`)
  }
  if (seed.truncated) failures.push("conversation.read truncated the long transcript")
  if (seed.messageCount !== seed.turns)
    failures.push(
      `conversation.read has ${seed.messageCount} messages, seeded ${seed.turns}`,
    )
  if (seed.viewHasLongText !== true)
    failures.push("conversation.read does not contain the long user text")
  if (typeof seed.replyText !== "string" || seed.replyText === "")
    failures.push("the stress scenario does not answer with text")
  const replies = Array.isArray(seed.replies) ? seed.replies : []
  if (replies.length !== seed.turns)
    failures.push(
      `conversation.read has ${replies.length} agent replies, seeded ${seed.turns}`,
    )
  if (replies.some((text) => text !== seed.replyText))
    failures.push("an agent reply is not the scripted text")
  return failures
}

/**
 * The layout's own cap was filled, the split chord arrived, and the extra
 * split did not add a pane. A frame over 50 ms is not one of these failures.
 */
export function capHeld({ filled, after, cap, chordArrived }) {
  const failures = []
  if (filled !== cap) failures.push(`opened ${filled} panes, the layout caps at ${cap}`)
  if (!chordArrived) failures.push("the split chord did not reach the page")
  if (after !== filled)
    failures.push(`a split past the cap left ${after} panes, the cap is ${cap}`)
  return failures
}

/** The session list has to show every seeded conversation. */
export function listedOnPage(wantIds, foundIds) {
  const found = Array.isArray(foundIds) ? foundIds : []
  return (wantIds ?? [])
    .filter((id) => !found.includes(id))
    .map((id) => `the session list does not show ${id}`)
}

/** The scroller was there, overflowed, and moved. */
export function scrollHeld(scrolled) {
  if (!scrolled?.found) return ["no transcript to scroll"]
  if (!scrolled.overflow) return ["the long transcript did not overflow its scroller"]
  if (!(scrolled.scrollTop > 0)) return ["the scroll did not move"]
  return []
}

/**
 * The open transcript is the seeded conversation, on screen, each user
 * bubble the seeded text, each agent bubble the scripted reply, and the
 * scroller moved. `userTexts` and `agentTexts` are those bubbles'
 * `textContent`, not a shorter visible slice.
 */
export function transcriptHeld({
  turns,
  longText,
  replyText,
  userTexts,
  agentTexts,
  openId,
  longId,
  scrolled,
  onScreen,
}) {
  const failures = []
  if (onScreen !== true) failures.push("the long transcript was not on screen")
  if (openId !== longId)
    failures.push(`the open session is ${openId ?? "none"}, seeded ${longId}`)
  const texts = Array.isArray(userTexts) ? userTexts : []
  if (texts.length !== turns)
    failures.push(`the transcript shows ${texts.length} user messages, seeded ${turns}`)
  if (texts.some((text) => text !== longText))
    failures.push("a user message is not the seeded text")
  const replies = Array.isArray(agentTexts) ? agentTexts : []
  if (replies.length !== turns)
    failures.push(`the transcript shows ${replies.length} agent replies, seeded ${turns}`)
  if (replies.some((text) => text !== replyText))
    failures.push("an agent reply is not the scripted text")
  failures.push(...scrollHeld(scrolled))
  return failures
}

async function say(client, conversationId, text, agent, create) {
  const before = create
    ? 0
    : (await client.conversation.read(conversationId)).messages.length
  if (create) await client.conversation.create({ conversationId, agent })
  await client.conversation.send(conversationId, text)
  const end = Date.now() + 90_000
  let last
  do {
    const view = await client.conversation.read(conversationId)
    last = view.messages.at(-1)
    if (view.messages.length > before && last && turnEnded(last.status)) {
      if (last.status !== "completed")
        throw new CannotRun(`the scripted turn ended ${last.status}`)
      return view
    }
    await sleep(200)
  } while (Date.now() < end)
  throw new CannotRun(
    `the scripted turn did not end; it was last ${last?.status ?? "absent"}`,
  )
}

/**
 * Creates `paneCount` conversations on the panel client. The last is the
 * long transcript (`gatewayStress.turns` copies of {@link longUserText}),
 * so a newest-first list opens it. Each earlier one is one short message
 * so the switcher has a distinct row.
 * Resolves with the seed `seedHeld` accepts, or throws `CannotRun` when the
 * gateway did not store that seed.
 */
export async function seedGatewayStress(client, agent, paneCount, gateway, replyText) {
  if (!Number.isInteger(paneCount) || paneCount < 1)
    throw new CannotRun(`pane cap ${paneCount} cannot seed a stress`)
  if (typeof replyText !== "string" || replyText === "")
    throw new CannotRun("the stress scenario does not answer with text")
  const longText = longUserText()
  const ids = []
  let longView
  try {
    for (let index = 0; index < paneCount; index += 1) {
      const conversationId = randomUUID()
      const long = index === paneCount - 1
      const text = long ? longText : `Alpha stress pane ${index + 1}`
      const turns = long ? gatewayStress.turns : 1
      let view
      for (let turn = 0; turn < turns; turn += 1) {
        const started = Date.now()
        view = await say(client, conversationId, text, agent, turn === 0)
        log(
          `seeded ${conversationId} turn ${turn + 1}/${turns} in ${Date.now() - started} ms`,
        )
      }
      ids.push(conversationId)
      if (long) longView = view
    }
    const list = await client.conversation.list({})
    const seed = {
      ids,
      longId: ids.at(-1),
      longText,
      turns: gatewayStress.turns,
      characters: longText.length,
      marker: gatewayStress.marker,
      listComplete: list.complete === true,
      listedCount: list.conversations.length,
      listedIds: list.conversations.map((row) => row.conversationId),
      truncated: longView.truncated === true,
      messageCount: longView.messages.length,
      viewHasLongText: longView.messages.every(
        (message) => message.userText === longText,
      ),
      replyText,
      replies: longView.messages.map((message) => storedReplyText(message)),
    }
    const failures = seedHeld(seed)
    if (failures.length > 0) throw new CannotRun(failures[0])
    return seed
  } catch (error) {
    if (gateway?.log) log(gateway.log().slice(-2000))
    throw error
  }
}
