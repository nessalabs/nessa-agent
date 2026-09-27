import { describe, expect, it } from "vitest"
import type { WorkspaceUpdate } from "../../application/ports"
import { WorkspaceSourceError } from "../../application/ports"
import { messageText, type Transcript } from "../../model/transcript"
import { inMemorySource, type Schedule } from "./in-memory-source"
import { scriptTiming } from "./scripted-replies"

/** A clock and timers the test moves by hand. */
function manualSchedule() {
  let now = 1_000_000
  let timers: { at: number; run: () => void; cancelled: boolean }[] = []
  const schedule: Schedule = {
    now: () => now,
    after(ms, run) {
      const timer = { at: now + ms, run, cancelled: false }
      timers.push(timer)
      return () => {
        timer.cancelled = true
      }
    },
  }
  const advance = (ms: number) => {
    const until = now + ms
    for (;;) {
      const due = timers
        .filter((timer) => !timer.cancelled && timer.at <= until)
        .sort((a, b) => a.at - b.at)[0]
      if (!due) break
      now = due.at
      due.cancelled = true
      due.run()
    }
    now = until
    timers = timers.filter((timer) => !timer.cancelled)
  }
  return { schedule, advance, pending: () => timers.filter((t) => !t.cancelled).length }
}

const flush = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve()
}

function started() {
  const clock = manualSchedule()
  const source = inMemorySource(clock.schedule)
  const updates: WorkspaceUpdate[] = []
  source.subscribe((update) => updates.push(update))
  return { ...clock, source, updates }
}

describe("the in-memory source", () => {
  it("offers the sample organisation and every session's conversation", async () => {
    const { source } = started()
    const organisation = await source.organisation()
    expect(organisation.sections.length).toBeGreaterThan(0)
    expect(organisation.sessions.length).toBeGreaterThan(10)
    const transcript = await source.transcript(organisation.sessions[0].id)
    expect(transcript.messages.length).toBeGreaterThan(0)
  })

  it("refuses a session it does not have, by type", async () => {
    const { source } = started()
    await expect(source.transcript("missing")).rejects.toMatchObject({
      reason: "unknown-session",
    })
    await expect(
      source.send({
        initiator: "person",
        sessionId: "missing",
        messageId: "m",
        text: "hi",
        model: { provider: "anthropic", modelId: "claude-opus-5" },
      }),
    ).rejects.toBeInstanceOf(WorkspaceSourceError)
  })

  it("thinks, reads around, then streams a reply a few words at a time and rests", async () => {
    const { source, updates, advance } = started()
    await source.send({
      initiator: "person",
      sessionId: "new",
      messageId: "m1",
      text: "Sketch a calmer header",
      model: { provider: "anthropic", modelId: "claude-opus-5" },
      start: { channelId: "desktop-app", title: "Sketch a calmer header" },
    })
    const latest = () =>
      updates.filter((update) => update.kind === "transcript").at(-1) as Extract<
        WorkspaceUpdate,
        { kind: "transcript" }
      >
    expect(updates[0]).toMatchObject({
      kind: "session",
      session: { id: "new", status: "running" },
    })
    expect(latest().transcript.activity?.label).toBe("Thinking")
    advance(scriptTiming.readingMs)
    expect(latest().transcript.activity?.label).toBe("Reading the workspace")
    advance(scriptTiming.answerMs - scriptTiming.readingMs + scriptTiming.stepMs)
    const first = latest().transcript
    expect(first.activity).toBeNull()
    expect(messageText(first.messages.at(-1)!).split(/\s+/).filter(Boolean)).toHaveLength(
      scriptTiming.wordsPerStep,
    )
    advance(10_000)
    const done = latest().transcript
    expect(messageText(done.messages.at(-1)!)).toMatch(/^On it\./)
    const summaries = updates.filter((update) => update.kind === "session")
    expect(summaries.at(-1)).toMatchObject({ session: { status: "idle" } })
  })

  it("keeps earlier messages as the same values while a reply streams", async () => {
    const { source, updates, advance } = started()
    await source.send({
      initiator: "person",
      sessionId: "split-panes",
      messageId: "m1",
      text: "More",
      model: { provider: "anthropic", modelId: "claude-opus-5" },
    })
    advance(scriptTiming.answerMs + scriptTiming.stepMs * 3)
    const transcripts = updates
      .filter((update) => update.kind === "transcript")
      .map(
        (update) =>
          (update as Extract<WorkspaceUpdate, { kind: "transcript" }>).transcript,
      )
    const [a, b] = transcripts.slice(-2)
    expect(a.messages[0]).toBe(b.messages[0])
  })

  it("runs an approved command, or lets it go when denied", async () => {
    const { source, updates, advance } = started()
    const before = await source.transcript("retry-budget")
    await source.approve("retry-budget", before.approval!.id, "once", "person")
    expect(updates.at(-1)).toMatchObject({
      kind: "transcript",
      transcript: { approval: null },
    })
    advance(scriptTiming.commandMs)
    const after = await source.transcript("retry-budget")
    expect(after.messages.at(-1)?.parts[0]).toMatchObject({ kind: "step", label: "Ran" })
    const waiting = await source.transcript("notarize")
    await source.deny("notarize", waiting.approval!.id, "agent")
    expect(messageText((await source.transcript("notarize")).messages.at(-1)!)).toMatch(
      /won’t run it/,
    )
    expect(source.audit()[0].before?.waitingOn).toEqual({
      approvalId: before.approval!.id,
      command: before.approval!.command,
    })
    // Taken, each names the revision it produced: the session's summary after it.
    const [allowed, denied] = source.audit()
    expect(allowed.after).toBe(allowed.before!.revision + 1)
    expect(denied.after).toBeGreaterThan(denied.before!.revision)
    expect(Object.isFrozen(allowed.before)).toBe(true)
    expect(Object.isFrozen(allowed.before?.waitingOn)).toBe(true)
    // Each decision is on record with who made it, and what became of it.
    expect(
      source.audit().map((d) => [d.sessionId, d.action, d.initiator, d.outcome]),
    ).toEqual([
      ["retry-budget", "allow-once", "person", "taken"],
      ["notarize", "deny", "agent", "taken"],
    ])
  })

  it("refuses an answer to an approval that is not waiting, and records it all the same", async () => {
    const { source } = started()
    await expect(
      source.approve("split-panes", "nothing", "once", "person"),
    ).rejects.toMatchObject({
      reason: "not-waiting",
    })
    expect(source.audit()).toMatchObject([
      {
        sessionId: "split-panes",
        approvalId: "nothing",
        action: "allow-once",
        initiator: "person",
        outcome: { refused: "not-waiting" },
      },
    ])
  })

  it("stops an old answer when the session is sent to again", async () => {
    const { source, advance, updates } = started()
    const model = { provider: "anthropic", modelId: "claude-opus-5" }
    await source.send({
      initiator: "person",
      sessionId: "frost",
      messageId: "m1",
      text: "one",
      model,
    })
    advance(scriptTiming.answerMs - 1)
    await source.send({
      initiator: "person",
      sessionId: "frost",
      messageId: "m2",
      text: "two",
      model,
    })
    advance(20_000)
    const replies = (await source.transcript("frost")).messages.filter(
      (message) => message.role === "agent" && messageText(message).includes("“one”"),
    )
    expect(replies).toHaveLength(0)
    expect(updates.length).toBeGreaterThan(0)
  })

  it("takes a message sent again under the same id once", async () => {
    const { source, pending, updates } = started()
    const model = { provider: "anthropic", modelId: "claude-opus-5" }
    await source.send({
      initiator: "person",
      sessionId: "frost",
      messageId: "m1",
      text: "one",
      model,
    })
    const timers = pending()
    const heard = updates.length
    await source.send({
      initiator: "person",
      sessionId: "frost",
      messageId: "m1",
      text: "one",
      model,
    })
    expect(pending()).toBe(timers)
    expect(updates).toHaveLength(heard)
    const held = (await source.transcript("frost")).messages.filter((m) => m.id === "m1")
    expect(held).toHaveLength(1)
  })

  it("never gives two contents one revision: a new session's first change follows what a read said", async () => {
    const { source } = started()
    const model = { provider: "anthropic", modelId: "claude-opus-5" }
    // A subscriber that reads as soon as the new session is listed, as the window does.
    let early: Promise<Transcript> | undefined
    source.subscribe((update) => {
      if (update.kind === "session" && update.session.id === "fresh" && !early)
        early = source.transcript("fresh")
    })
    await source.send({
      initiator: "person",
      sessionId: "fresh",
      messageId: "m1",
      text: "one",
      model,
      start: { channelId: "desktop-app", title: "One" },
    })
    const first = await early
    const after = await source.transcript("fresh")
    // Whatever the read saw, one revision names one content.
    const byRevision = new Map<number, Transcript>()
    for (const seen of [first, after]) {
      if (!seen) continue
      const held = byRevision.get(seen.revision)
      if (held) expect(seen).toEqual(held)
      byRevision.set(seen.revision, seen)
    }
    expect(after.messages.map((message) => message.id)).toEqual(["m1"])
  })

  it("lets an approval go when the session is sent to instead, so it rests with nothing asked", async () => {
    const { source, advance } = started()
    const model = { provider: "anthropic", modelId: "claude-opus-5" }
    const asked = (await source.transcript("retry-budget")).approval
    expect(asked).not.toBeNull()
    await source.send({
      initiator: "agent",
      sessionId: "retry-budget",
      messageId: "m1",
      text: "hello",
      model,
    })
    // On record: the approval let go, by whom, from what, to what.
    const [letGo] = source.audit()
    expect(letGo).toMatchObject({
      action: "let-go",
      approvalId: asked?.id,
      messageId: "m1",
      initiator: "agent",
      outcome: "taken",
      before: { waitingOn: { approvalId: asked?.id } },
    })
    expect(letGo.after).toBeGreaterThan(letGo.before!.revision)
    advance(60_000)
    expect((await source.transcript("retry-budget")).approval).toBeNull()
    await expect(
      source.approve("retry-budget", asked?.id ?? "", "once", "person"),
    ).rejects.toThrow(WorkspaceSourceError)
  })

  it("records a call it carried out as taken, whatever a listener does with the update", async () => {
    const { source } = started()
    const error = console.error
    console.error = () => {}
    const heard: string[] = []
    source.subscribe(() => {
      throw new Error("a listener's own fault")
    })
    source.subscribe((update) => heard.push(update.kind))
    try {
      await source.setPinned("frost", false, "person")
    } finally {
      console.error = error
    }
    expect(source.audit().at(-1)?.outcome).toBe("taken")
    expect(heard).toContain("session")
  })

  it("begins no session again under an archived id", async () => {
    const { source } = started()
    const model = { provider: "anthropic", modelId: "claude-opus-5" }
    await source.archive("frost", "person")
    await expect(
      source.send({
        initiator: "person",
        sessionId: "frost",
        messageId: "m1",
        text: "again",
        model,
        start: { channelId: "desktop-app", title: "Again" },
      }),
    ).rejects.toThrow(WorkspaceSourceError)
  })

  it("pins, archives and marks read, emitting each change", async () => {
    const { source, updates } = started()
    await source.setPinned("frost", false, "agent")
    expect(updates.at(-1)).toMatchObject({
      kind: "session",
      session: { id: "frost", pinned: false },
    })
    await source.markRead("signing")
    expect(updates.at(-1)).toMatchObject({ kind: "session", session: { unread: false } })
    await source.archive("frost", "person")
    expect(updates.at(-1)).toMatchObject({ kind: "session-removed", sessionId: "frost" })
    expect(source.audit().map((d) => [d.action, d.initiator, d.outcome])).toEqual([
      ["unpin", "agent", "taken"],
      ["archive", "person", "taken"],
    ])
    // Each with the session as it stood when asked, and when it settled.
    const [unpin, archive] = source.audit()
    expect(unpin.before).toMatchObject({ pinned: true })
    expect(archive.before).toMatchObject({ pinned: false })
    expect(archive.before?.revision).toBeGreaterThan(unpin.before?.revision ?? 0)
    expect(unpin.settledAt).toBeDefined()
    await expect(source.transcript("frost")).rejects.toMatchObject({
      reason: "unknown-session",
    })
  })

  it("records each call against the session as the source found it, calls made at once included", async () => {
    const { source } = started()
    const first = source.setPinned("signing", true, "agent")
    const second = source.setPinned("signing", true, "person")
    const archiving = source.archive("signing", "person")
    const late = source.setPinned("signing", false, "agent")
    await Promise.all([first, second, archiving])
    await expect(late).rejects.toMatchObject({ reason: "unknown-session" })
    const [one, two, archive, unpin] = source.audit()
    expect(one.before).toMatchObject({ pinned: false })
    expect(two.before).toMatchObject({ pinned: true, revision: one.after })
    expect(archive.before?.revision).toBe(two.after)
    expect(archive.after).toBe((two.after ?? 0) + 1)
    expect(unpin).toMatchObject({ before: null, outcome: { refused: "unknown-session" } })
    expect(Object.isFrozen(unpin)).toBe(true)
  })

  it("keeps its record to itself, and records a call a disposed source refuses", async () => {
    const { source } = started()
    const first = source.audit()
    source.dispose()
    await expect(source.deny("notarize", "any", "agent")).rejects.toMatchObject({
      reason: "unavailable",
    })
    expect(first).toEqual([])
    expect(source.audit()).toMatchObject([
      { sessionId: "notarize", action: "deny", outcome: { refused: "unavailable" } },
    ])
    await expect(source.organisation()).rejects.toMatchObject({ reason: "unavailable" })
    await expect(source.transcript("notarize")).rejects.toMatchObject({
      reason: "unavailable",
    })
  })

  it("stops every script and emits nothing once disposed", async () => {
    const { source, updates, advance, pending } = started()
    await source.send({
      initiator: "person",
      sessionId: "frost",
      messageId: "m1",
      text: "one",
      model: { provider: "anthropic", modelId: "claude-opus-5" },
    })
    const count = updates.length
    source.dispose()
    expect(pending()).toBe(0)
    advance(20_000)
    await flush()
    expect(updates).toHaveLength(count)
    // Nor does a call made afterwards schedule anything.
    await expect(
      source.send({
        initiator: "person",
        sessionId: "frost",
        messageId: "m2",
        text: "two",
        model: { provider: "anthropic", modelId: "claude-opus-5" },
      }),
    ).rejects.toMatchObject({ reason: "unavailable" })
    expect(pending()).toBe(0)
  })
})
