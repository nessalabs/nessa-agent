import { describe, expect, it } from "vitest"
import type { ComposerModel } from "../../../model/composer-options"
import { titleFrom } from "../../model/transcript"
import { contradicts } from "../../model/workspace-index"
import { inMemorySource, type Schedule } from "./in-memory-source"
import {
  maxMessageCharacters,
  seededWorkspace,
  seededWorkspaceSpec,
  SeededWorkspaceRefusal,
  type SeededWorkspaceSpec,
} from "./seeded-workspace"

const now = 1_700_000_000_000

const quiet: Schedule = {
  now: () => now,
  after: () => () => undefined,
}

function spec(over: Partial<SeededWorkspaceSpec> = {}): SeededWorkspaceSpec {
  return {
    seed: 590,
    now,
    sessions: 4,
    longTranscripts: 1,
    messages: 3,
    messageCharacters: 120,
    ...over,
  }
}

function reasonOf(
  over: Partial<SeededWorkspaceSpec>,
  models?: readonly ComposerModel[],
): SeededWorkspaceRefusal["reason"] {
  try {
    if (models === undefined) seededWorkspace(spec(over))
    else seededWorkspace(spec(over), models)
  } catch (error) {
    if (error instanceof SeededWorkspaceRefusal) return error.reason
    throw error
  }
  throw new Error("the builder accepted a spec it should refuse")
}

describe("seeded workspace", () => {
  it("refuses a spec that is not a whole seeded run", () => {
    const cases: Array<[Partial<SeededWorkspaceSpec>, SeededWorkspaceRefusal["reason"]]> =
      [
        [{ seed: 1.5 }, "seed"],
        [{ seed: -1 }, "seed"],
        [{ seed: 0x1_0000_0000 }, "seed"],
        [{ now: Number.NaN }, "now"],
        [{ now: 1e20 }, "now"],
        [{ now: -1e20 }, "now"],
        [{ now: -8_640_000_000_000_000 }, "now"],
        [
          {
            now: -8_640_000_000_000_000 + 60 * 60_000 - 1,
            sessions: 1,
            longTranscripts: 0,
            messages: 0,
            messageCharacters: 0,
          },
          "now",
        ],
        [{ sessions: -1 }, "sessions"],
        [{ sessions: 1.2 }, "sessions"],
        [{ longTranscripts: 5 }, "longTranscripts"],
        [{ longTranscripts: 0, messages: 2 }, "messages"],
        [{ messages: 0 }, "messages"],
        [{ messages: 1, messageCharacters: 0 }, "messages"],
        [{ messages: 1, messageCharacters: 8 }, "messages"],
        [{ longTranscripts: 0, messages: 0, messageCharacters: 8 }, "messageCharacters"],
        [{ messageCharacters: -1 }, "messageCharacters"],
        [{ messageCharacters: 1e20 }, "messageCharacters"],
        [{ messageCharacters: maxMessageCharacters + 1 }, "messageCharacters"],
      ]
    for (const [over, reason] of cases) expect(reasonOf(over)).toBe(reason)
  })

  it("formats stored times at the edge of the date range", () => {
    const limit = 8_640_000_000_000_000
    const built = seededWorkspace(
      spec({
        now: -limit + 79 * 60_000,
        sessions: 1,
        longTranscripts: 1,
        messages: 80,
        messageCharacters: 40,
      }),
    )
    const session = built.index.sessions[0]
    expect(session?.startedAt).toBe(-limit)
    expect(session?.updatedAt).toBe(-limit + 79 * 60_000)
    const times = [
      session?.updatedAt,
      session?.startedAt,
      ...(built.transcripts.get("load-00000")?.messages.map((message) => message.at) ??
        []),
    ]
    for (const at of times)
      expect(() => new Date(at ?? Number.NaN).toISOString()).not.toThrow()
  })

  it("refuses an empty model catalogue", () => {
    expect(reasonOf({}, [])).toBe("catalogue")
  })

  it("builds ten thousand summaries through the index keeper and the in-memory source", async () => {
    const asked = spec({
      sessions: 10_000,
      longTranscripts: 1,
      messages: 8,
      messageCharacters: 4_000,
    })
    const built = seededWorkspace(asked)
    const again = seededWorkspace(asked)
    expect(contradicts(built.contradictions)).toBe(false)
    expect(built.index.sessions).toHaveLength(10_000)
    expect(built.transcripts.size).toBe(10_000)
    expect(built.report.sessions).toBe(10_000)
    expect(built.report.channels).toBe(4)
    expect(built.report.largestChannelSessions).toBe(2_500)
    expect(built.report.channelSessions["load-desktop"]).toBe(2_500)
    expect(built.report.channelSessions["load-release"]).toBe(2_500)
    expect(built.report.channelSessions["load-reading"]).toBe(2_500)
    expect(built.report.channelSessions["load-home"]).toBe(2_500)
    expect(built.report.channelNames["load-desktop"]).toBe("desktop")
    expect(built.report.generator).toBe("seeded-workspace")
    expect(built.report.algorithm).toBe("mulberry32")
    expect(built.report.longTranscripts).toBe(1)
    expect(built.report.messages).toBe(8)
    expect(built.report.longPlainTextCharacters).toBe(4_000)
    expect(
      built.report.statusCounts.idle +
        built.report.statusCounts.running +
        built.report.statusCounts.needsYou,
    ).toBe(10_000)

    const utf8 = new TextEncoder()
    let maxTitle = 0
    let maxPreview = 0
    const titles: string[] = []
    const counts = { idle: 0, running: 0, needsYou: 0 }
    for (const session of built.index.sessions) {
      titles.push(session.title)
      expect(built.transcripts.has(session.id)).toBe(true)
      const held = built.transcripts.get(session.id)
      const opening = held?.messages[0]?.parts[0]
      expect(opening?.kind).toBe("text")
      if (opening?.kind === "text") expect(session.title).toBe(titleFrom(opening.text))
      const last = held?.messages.at(-1)
      let said = ""
      if (last) for (const part of last.parts) if (part.kind === "text") said = part.text
      expect(session.preview).toBe(said)
      if (session.title.length > maxTitle) maxTitle = session.title.length
      const previewBytes = utf8.encode(session.preview).byteLength
      if (previewBytes > maxPreview) maxPreview = previewBytes
      if (session.status === "needs-you") counts.needsYou += 1
      else counts[session.status] += 1
    }
    expect(built.report.maxTitleCharacters).toBe(maxTitle)
    expect(built.report.maxPreviewUtf8Bytes).toBe(maxPreview)
    expect(built.report.statusCounts).toEqual(counts)
    expect(again.index.sessions.map((session) => session.title)).toEqual(titles)
    expect(again.index.sessions.map((session) => session.preview)).toEqual(
      built.index.sessions.map((session) => session.preview),
    )

    const long = built.transcripts.get("load-00000")
    expect(again.transcripts.get("load-00000")).toEqual(long)
    expect(long?.messages).toHaveLength(8)
    const newest = built.index.sessions[0]
    expect(long?.messages.at(-1)?.at).toBe(newest?.updatedAt)
    expect(long?.messages[0]?.at).toBe((newest?.updatedAt ?? 0) - 7 * 60_000)
    expect(newest?.startedAt).toBeLessThanOrEqual(long?.messages[0]?.at ?? 0)
    expect((long?.messages.at(-1)?.at ?? 0) <= asked.now).toBe(true)
    const longPart = long?.messages
      .flatMap((message) => message.parts)
      .find((part) => part.kind === "text" && part.text.length === 4_000)
    expect(longPart?.kind).toBe("text")
    if (longPart?.kind === "text") {
      expect(utf8.encode(longPart.text).byteLength).toBe(4_000)
      expect(newest?.preview).toBe(longPart.text)
    }
    const marked = long?.messages
      .flatMap((message) => message.parts)
      .some((part) => part.kind === "text" && part.text.includes("**backup**"))
    const code = long?.messages
      .flatMap((message) => message.parts)
      .some((part) => part.kind === "code")
    expect(marked).toBe(true)
    expect(code).toBe(true)

    let recounted = 0
    if (long)
      for (const message of long.messages)
        for (const part of message.parts) {
          if (part.kind === "text") recounted += utf8.encode(part.text).byteLength
          else if (part.kind === "code") recounted += utf8.encode(part.code).byteLength
          else if (part.kind === "step") {
            recounted += utf8.encode(part.label).byteLength
            if (part.detail !== undefined)
              recounted += utf8.encode(part.detail).byteLength
          }
        }
    expect(built.report.longTranscriptUtf8Bytes).toBe(recounted)

    const source = inMemorySource(quiet, {
      index: built.index,
      transcripts: new Map(built.transcripts),
    })
    expect((await source.index()).sessions).toHaveLength(10_000)
    expect((await source.transcript("load-00000")).messages).toHaveLength(8)
    source.dispose()
  })

  it("ends a long transcript at updatedAt and starts the session no later than the first message", () => {
    const built = seededWorkspace(
      spec({ sessions: 1, longTranscripts: 1, messages: 80, messageCharacters: 40 }),
    )
    const session = built.index.sessions[0]
    const messages = built.transcripts.get("load-00000")?.messages
    expect(messages).toHaveLength(80)
    expect(session?.updatedAt).toBe(now)
    expect(messages?.at(-1)?.at).toBe(session?.updatedAt)
    expect(messages?.[0]?.at).toBe(now - 79 * 60_000)
    expect(session?.startedAt).toBe(messages?.[0]?.at)
    for (let index = 1; index < (messages?.length ?? 0); index += 1)
      expect((messages?.[index]?.at ?? 0) - (messages?.[index - 1]?.at ?? 0)).toBe(60_000)
  })

  it("stores a plain part at the length the spec asked for", () => {
    const built = seededWorkspace(
      spec({ sessions: 1, longTranscripts: 1, messages: 2, messageCharacters: 9_000 }),
    )
    const last = built.transcripts.get("load-00000")?.messages.at(-1)
    const tail = last?.parts.filter((part) => part.kind === "text").at(-1)
    expect(tail?.kind === "text" ? tail.text.length : 0).toBe(9_000)
    expect(built.index.sessions[0]?.preview).toBe(tail?.kind === "text" ? tail.text : "")
    expect(built.report.longPlainTextCharacters).toBe(9_000)
  })

  it("changes a title when the seed changes and records an empty long part", () => {
    const quietSpec = spec({
      sessions: 8,
      longTranscripts: 1,
      messages: 2,
      messageCharacters: 0,
    })
    const first = seededWorkspace(quietSpec)
    const other = seededWorkspace({ ...quietSpec, seed: 591 })
    expect(first.report.longPlainTextCharacters).toBe(0)
    expect(first.index.sessions[0]?.preview).toBe("")
    expect(
      other.index.sessions.some(
        (session, index) => session.title !== first.index.sessions[index]?.title,
      ),
    ).toBe(true)
  })

  it("reads the dry-run query and refuses a field that is not a whole decimal", () => {
    const query =
      "?seeded=590&now=1700000000000&sessions=10000&longTranscripts=1&messages=8&messageCharacters=4000"
    expect(seededWorkspaceSpec(query)).toEqual({
      seed: 590,
      now: 1_700_000_000_000,
      sessions: 10_000,
      longTranscripts: 1,
      messages: 8,
      messageCharacters: 4_000,
    })
    expect(seededWorkspaceSpec("")).toBeNull()
    expect(seededWorkspaceSpec("?gateway=1")).toBeNull()
    const refused = (search: string) => {
      try {
        seededWorkspaceSpec(search)
      } catch (error) {
        if (error instanceof SeededWorkspaceRefusal) return error.reason
        throw error
      }
      throw new Error("the query was accepted")
    }
    expect(refused("?seeded")).toBe("seed")
    expect(
      refused(
        "?seeded=01&now=1&sessions=0&longTranscripts=0&messages=0&messageCharacters=0",
      ),
    ).toBe("seed")
    expect(refused("?seeded=590")).toBe("now")
    expect(
      refused(
        "?seeded=590&now=1e20&sessions=0&longTranscripts=0&messages=0&messageCharacters=0",
      ),
    ).toBe("now")
    expect(
      refused(
        "?seeded=590&now=1700000000000&sessions=1&longTranscripts=2&messages=2&messageCharacters=1",
      ),
    ).toBe("longTranscripts")
    const repeated =
      "&now=1700000000000&sessions=0&longTranscripts=0&messages=0&messageCharacters=0"
    expect(refused(`?seeded=01&seeded=590${repeated}`)).toBe("seed")
    expect(refused(`?seeded=590&seeded=01${repeated}`)).toBe("seed")
    expect(refused(`?seeded=590&seeded=590${repeated}`)).toBe("seed")
    expect(
      refused(
        "?seeded=590&now=1700000000000&sessions=10000&sessions=1&longTranscripts=1&messages=8&messageCharacters=4000",
      ),
    ).toBe("sessions")
    expect(
      refused(
        "?seeded=590&now=1700000000000&sessions=1&sessions=10000&longTranscripts=1&messages=8&messageCharacters=4000",
      ),
    ).toBe("sessions")
    expect(
      seededWorkspaceSpec(
        "??seeded=590&now=1700000000000&sessions=0&longTranscripts=0&messages=0&messageCharacters=0",
      ),
    ).toBeNull()
  })
})
