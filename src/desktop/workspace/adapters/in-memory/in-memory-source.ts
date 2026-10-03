/**
 * A `WorkspaceSource` held in memory: the sample workspace, and agents that
 * answer from a script. A message sent is thought about, then read around,
 * then answered a couple of words at a time; an approval runs its command or
 * is let go. The gateway's source (`../gateway/`) implements the same port;
 * this one is what the window shows without a gateway — the verification
 * fixtures and previews — and the only place sample data and scripted
 * replies exist.
 *
 * The timers are its own, taken from `schedule`, and `dispose` cancels every
 * one; a session sent to again while it is still answering drops the old
 * answer. What it holds is replaced, never changed: every update it emits is
 * a new value, and a value it has handed out stays as it was.
 */
import type { SessionSummary } from "../../model/workspace-index"
import {
  emptyTranscript,
  type Message,
  type Part,
  type Transcript,
} from "../../model/transcript"
import {
  failureReason,
  WorkspaceSourceError,
  type ApprovalScope,
  type Initiator,
  type OutgoingMessage,
  type WorkspaceSource,
  type WorkspaceUpdate,
} from "../../application/ports"
import type { WorkspaceFailureReason } from "../../model/failure"
import { sampleWorkspace } from "./sample-workspace"
import {
  approvedReply,
  deniedReply,
  replyTo,
  runningNow,
  scriptTiming,
} from "./scripted-replies"

export interface Schedule {
  now(): number
  /** Runs `run` after `ms`; the returned function cancels it. */
  after(ms: number, run: () => void): () => void
}

/** A session as it stood for an audited call: its revision, pin and the approval it waited on. */
export interface AuditedState {
  readonly revision: number
  readonly pinned: boolean
  readonly waitingOn: { readonly approvalId: string; readonly command: string } | null
}

/**
 * A consequential call, as the source recorded it: what was asked, of which
 * session, by whom and when — on record the moment it is asked — then the
 * session as it stood when the source carried it out (`before`), what became
 * of it and when, and, taken, the revision it produced (`after`: the new
 * summary's, or the removal's). This source refuses only by type; it has no
 * faults of its own to tell apart. Kept in memory only, for as long as the
 * source lives; entries are frozen and replaced, never changed.
 */
export interface AuditEntry {
  readonly sessionId: string
  /** `let-go`: a message sent while an approval waited moved the turn on without it. */
  readonly action:
    "allow-once" | "allow-always" | "deny" | "pin" | "unpin" | "archive" | "let-go"
  /** The approval answered, or let go. */
  readonly approvalId?: string
  /** The message that let an approval go, for a `let-go`. */
  readonly messageId?: string
  readonly initiator: Initiator
  readonly at: number
  /** The session when the source came to carry the call out, if it held it; absent until then. */
  readonly before?: AuditedState | null
  readonly outcome: "asked" | "taken" | { readonly refused: WorkspaceFailureReason }
  /** The revision the call produced, once taken. */
  readonly after?: number
  readonly settledAt?: number
}

export interface InMemorySource extends WorkspaceSource {
  /** Every consequential call, in order — answers, pins, archives — refused ones included. */
  audit(): readonly AuditEntry[]
  /** Stops every scripted reply and refuses every later call; nothing is emitted afterwards. */
  dispose(): void
}

export function inMemorySource(
  schedule: Schedule,
  seed: ReturnType<typeof sampleWorkspace> = sampleWorkspace(schedule.now()),
): InMemorySource {
  // Archived ids: none is begun again, so no revision of one counts from 1 twice.
  const archived = new Set<string>()
  const sessions = new Map(seed.index.sessions.map((session) => [session.id, session]))
  const transcripts = new Map(seed.transcripts)
  const listeners = new Set<(update: WorkspaceUpdate) => void>()
  // Each session's running script, so a new message or `dispose` can stop it.
  const scripts = new Map<string, Set<() => void>>()
  let disposed = false
  // A disposed source answers nothing: every later call is refused, and none schedules.
  const live = <T>(work: () => T): Promise<T> =>
    run(() => {
      if (disposed) throw new WorkspaceSourceError("unavailable")
      return work()
    })
  // On record the moment it is asked — a refused call, a disposed source's,
  // too — then given the session as the source found it, and what became of
  // it. Entries are frozen and replaced, never changed.
  let entries: readonly AuditEntry[] = Object.freeze([])
  const stateOf = (sessionId: string): AuditedState | null => {
    const held = sessions.get(sessionId)
    if (!held) return null
    const approval = transcripts.get(sessionId)?.approval
    return Object.freeze({
      revision: held.revision,
      pinned: held.pinned,
      waitingOn: approval
        ? Object.freeze({ approvalId: approval.id, command: approval.command })
        : null,
    })
  }
  /** Runs a consequential call; `work` answers with the revision it produced. */
  const audited = (
    asked: Pick<AuditEntry, "sessionId" | "action" | "approvalId" | "initiator">,
    work: () => number,
  ): Promise<void> => {
    let entry: AuditEntry = Object.freeze({
      ...asked,
      at: schedule.now(),
      outcome: "asked",
    })
    entries = Object.freeze([...entries, entry])
    const replace = (change: Partial<AuditEntry>) => {
      const next: AuditEntry = Object.freeze({ ...entry, ...change })
      entries = Object.freeze(entries.map((each) => (each === entry ? next : each)))
      entry = next
    }
    return live(() => {
      replace({ before: stateOf(asked.sessionId) })
      return work()
    }).then(
      (after) => replace({ outcome: "taken", after, settledAt: schedule.now() }),
      (error: unknown) => {
        replace({ outcome: { refused: failureReason(error) }, settledAt: schedule.now() })
        throw error
      },
    )
  }

  const emit = (update: WorkspaceUpdate) => {
    if (disposed) return
    // A listener's fault is its own: it neither stops the others nor turns a
    // call the source carried out into one it refused.
    for (const listener of listeners) {
      try {
        listener(update)
      } catch (error) {
        console.error("A workspace listener failed", error)
      }
    }
  }

  const known = (sessionId: string): SessionSummary => {
    const session = sessions.get(sessionId)
    if (!session) throw new WorkspaceSourceError("unknown-session")
    return session
  }

  // Every change the source makes to a session is a new revision of it.
  const putSession = (change: SessionSummary): SessionSummary => {
    const session = {
      ...change,
      revision: (sessions.get(change.id)?.revision ?? 0) + 1,
    }
    sessions.set(session.id, session)
    emit({ kind: "session", session })
    return session
  }

  const putTranscript = (change: Transcript) => {
    const transcript = {
      ...change,
      // Counted from what a read would have said, so no two contents share a revision.
      revision: transcriptOf(change.sessionId).revision + 1,
    }
    transcripts.set(transcript.sessionId, transcript)
    emit({ kind: "transcript", transcript })
  }

  const transcriptOf = (sessionId: string): Transcript =>
    // A session with no history yet: still the source's word, so revision 1.
    transcripts.get(sessionId) ?? { ...emptyTranscript(sessionId), revision: 1 }

  const stopScript = (sessionId: string) => {
    for (const cancel of scripts.get(sessionId) ?? []) cancel()
    scripts.delete(sessionId)
  }

  /** Runs `run` after `ms` as part of a session's script. */
  const later = (sessionId: string, ms: number, run: () => void) => {
    const timers = scripts.get(sessionId) ?? new Set<() => void>()
    scripts.set(sessionId, timers)
    const cancel = schedule.after(ms, () => {
      timers.delete(cancel)
      run()
    })
    timers.add(cancel)
  }

  const agentMessage = (sessionId: string, parts: readonly Part[]): Message => ({
    id: `${sessionId}-${transcriptOf(sessionId).messages.length + 1}`,
    role: "agent",
    at: schedule.now(),
    parts,
  })

  /** Ends a script: the agent's message in place, and the session at rest, with nothing going on. */
  const settle = (sessionId: string, preview: string) => {
    putSession({
      ...known(sessionId),
      status: "idle",
      preview,
      now: undefined,
      updatedAt: schedule.now(),
    })
    stopScript(sessionId)
  }

  /** What is going on in a session now, said with its summary's next revision. */
  const sayNow = (sessionId: string, now: string) => {
    const session = known(sessionId)
    if (session.now !== now) putSession({ ...session, now })
  }

  /** Streams `reply` into a new agent message, a couple of words at a time. */
  const stream = (
    sessionId: string,
    steps: readonly Part[],
    reply: string,
    now: string,
  ) => {
    const words = reply.split(/(?<=\s)/)
    const message = agentMessage(sessionId, [...steps, { kind: "text", text: "" }])
    const base = transcriptOf(sessionId)
    const write = (shown: number) => {
      const current = transcriptOf(sessionId)
      const written: Message = {
        ...message,
        parts: [...steps, { kind: "text", text: words.slice(0, shown).join("") }],
      }
      const others = current.messages.filter((other) => other.id !== message.id)
      putTranscript({ ...current, activity: null, messages: [...others, written] })
    }
    putTranscript({ ...base, activity: null, messages: [...base.messages, message] })
    sayNow(sessionId, now)
    const last = [...steps].reverse().find((part) => part.kind === "step")
    if (last?.kind === "step") previewStep(sessionId, last.label)
    const step = (shown: number) => {
      const next = Math.min(shown + scriptTiming.wordsPerStep, words.length)
      write(next)
      if (next === words.length) settle(sessionId, reply)
      else later(sessionId, scriptTiming.stepMs, () => step(next))
    }
    later(sessionId, scriptTiming.stepMs, () => step(0))
  }

  /**
   * What a running session is at, as its list row previews it: the step it
   * is on, rather than the message that started it. Its place in the lists
   * (`updatedAt`) stays where it is.
   */
  const previewStep = (sessionId: string, step: string) => {
    const session = known(sessionId)
    if (session.preview !== step) putSession({ ...session, preview: step })
  }

  const setActivity = (sessionId: string, label: string, now: string) => {
    putTranscript({
      ...transcriptOf(sessionId),
      activity: { label, since: schedule.now() },
    })
    previewStep(sessionId, label)
    sayNow(sessionId, now)
  }

  /** The channel a session is in — or, not begun yet, the one it starts in. */
  const channelName = (sessionId: string, starting?: string) =>
    seed.index.channels.find(
      (channel) => channel.id === (sessions.get(sessionId)?.channelId ?? starting),
    )?.name ?? ""

  const accept = (message: OutgoingMessage) => {
    const at = schedule.now()
    const existing = sessions.get(message.sessionId)
    if ((!existing && !message.start) || archived.has(message.sessionId))
      throw new WorkspaceSourceError("unknown-session")
    // Taken once: the same message sent again is already here.
    if (
      transcriptOf(message.sessionId).messages.some(
        (held) => held.id === message.messageId,
      )
    )
      return
    const found = stateOf(message.sessionId)
    const reply = replyTo(
      message.text,
      channelName(message.sessionId, message.start?.channelId),
    )
    const session: SessionSummary = existing
      ? {
          ...existing,
          status: "running",
          preview: message.text,
          now: reply.now.thinking,
          updatedAt: at,
          model: message.model,
        }
      : {
          id: message.sessionId,
          channelId: message.start?.channelId ?? "",
          title: message.start?.title ?? message.text,
          model: message.model,
          status: "running",
          startedAt: at,
          updatedAt: at,
          preview: message.text,
          now: reply.now.thinking,
          pinned: false,
          unread: false,
          revision: 0,
        }
    stopScript(session.id)
    putSession(session)
    const transcript = transcriptOf(session.id)
    const written: Message = {
      id: message.messageId,
      role: "user",
      at,
      parts: [{ kind: "text", text: message.text }],
    }
    // An approval the message lets go is on record, with who sent it, before
    // anyone hears of it: a subscriber shown the approval gone finds its
    // `let-go` in the audit already.
    if (found?.waitingOn)
      entries = Object.freeze([
        ...entries,
        Object.freeze({
          sessionId: session.id,
          action: "let-go",
          approvalId: found.waitingOn.approvalId,
          messageId: message.messageId,
          initiator: message.initiator,
          at,
          before: found,
          outcome: "taken",
          after: sessions.get(session.id)?.revision,
          settledAt: schedule.now(),
        } satisfies AuditEntry),
      ])
    putTranscript({
      ...transcript,
      messages: [
        ...transcript.messages.filter((other) => other.id !== written.id),
        written,
      ],
      // A new message moves the turn on: an approval still asked is let go.
      approval: null,
      activity: { label: "Thinking", since: at },
    })
    later(session.id, scriptTiming.readingMs, () =>
      setActivity(session.id, "Reading the workspace", reply.now.reading),
    )
    later(session.id, scriptTiming.answerMs, () =>
      stream(session.id, reply.steps, reply.text, reply.now.writing),
    )
  }

  const waiting = (sessionId: string, approvalId: string) => {
    known(sessionId)
    const transcript = transcriptOf(sessionId)
    if (transcript.approval?.id !== approvalId)
      throw new WorkspaceSourceError("not-waiting")
    return transcript
  }

  return {
    index: () =>
      live(() => ({
        sections: seed.index.sections,
        channels: seed.index.channels,
        sessions: [...sessions.values()],
      })),
    transcript: (sessionId) =>
      live(() => {
        known(sessionId)
        return transcriptOf(sessionId)
      }),
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    send: (message) => live(() => accept(message)),
    approve: (
      sessionId: string,
      approvalId: string,
      scope: ApprovalScope,
      initiator: Initiator,
    ) =>
      audited(
        {
          sessionId,
          approvalId,
          action: scope === "always" ? "allow-always" : "allow-once",
          initiator,
        },
        () => {
          const transcript = waiting(sessionId, approvalId)
          const command = transcript.approval?.command ?? ""
          putSession({
            ...known(sessionId),
            status: "running",
            now: runningNow(command),
            updatedAt: schedule.now(),
          })
          putTranscript({
            ...transcript,
            approval: null,
            activity: {
              label: `Running ${command.split(" ").slice(0, 2).join(" ")}`,
              since: schedule.now(),
            },
          })
          later(sessionId, scriptTiming.commandMs, () => {
            const reply = approvedReply(command, scope)
            const current = transcriptOf(sessionId)
            putTranscript({
              ...current,
              activity: null,
              messages: [...current.messages, agentMessage(sessionId, reply.parts)],
            })
            settle(sessionId, reply.preview)
          })
          return known(sessionId).revision
        },
      ),
    deny: (sessionId, approvalId, initiator) =>
      audited({ sessionId, approvalId, action: "deny", initiator }, () => {
        const transcript = waiting(sessionId, approvalId)
        const reply = deniedReply(transcript.approval?.command ?? "")
        putTranscript({
          ...transcript,
          approval: null,
          activity: null,
          messages: [...transcript.messages, agentMessage(sessionId, reply.parts)],
        })
        settle(sessionId, reply.preview)
        return known(sessionId).revision
      }),
    setPinned: (sessionId, pinned, initiator) =>
      audited(
        { sessionId, action: pinned ? "pin" : "unpin", initiator },
        () => putSession({ ...known(sessionId), pinned }).revision,
      ),
    archive: (sessionId, initiator) =>
      audited({ sessionId, action: "archive", initiator }, () => {
        // The removal is the summary's next revision (`WorkspaceUpdate`).
        const revision = known(sessionId).revision + 1
        stopScript(sessionId)
        sessions.delete(sessionId)
        archived.add(sessionId)
        emit({ kind: "session-removed", sessionId, revision })
        return revision
      }),
    markRead: (sessionId) =>
      live(() => {
        const session = known(sessionId)
        if (session.unread) putSession({ ...session, unread: false })
      }),
    audit: () => entries,
    dispose() {
      disposed = true
      for (const sessionId of [...scripts.keys()]) stopScript(sessionId)
      listeners.clear()
    },
  }
}

/** Settles a call the way a remote one would: later, and rejecting rather than throwing. */
function run<T>(work: () => T): Promise<T> {
  return new Promise((resolve, reject) => {
    queueMicrotask(() => {
      try {
        resolve(work())
      } catch (error) {
        reject(error)
      }
    })
  })
}
