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
import { conversationTitle } from "@nessa/client"
import type { SessionSummary } from "../../model/workspace-index"
import {
  atWork,
  emptyTranscript,
  type Approval,
  type ApprovalChoice,
  type Message,
  type MessageApp,
  type Part,
  type QuestionChoice,
  type Transcript,
  type TranscriptQuestion,
  type TurnMessage,
} from "../../model/transcript"
import {
  failureReason,
  WorkspaceSourceError,
  WorkspaceModelError,
  type ApprovalScope,
  type Initiator,
  type OutgoingMessage,
  type WorkspaceSource,
  type WorkspaceUpdate,
} from "../../application/ports"
import type { WorkspaceFailureReason } from "../../model/failure"
import type { AccessMode } from "../../model/access"
import { acceptedQuestionChoices } from "./question-answer"
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

/**
 * A session as it stood for an audited call: its revision, pin, the approval
 * it waited on, the turn at work, and the access its agent ran at. `turn` is
 * what a `stop` cancels, and `access` what a `set-access` changed from (the
 * entry's own `access` is what it asked for).
 */
export interface AuditedState {
  readonly revision: number
  readonly pinned: boolean
  readonly waitingOn: { readonly approvalId: string; readonly command: string } | null
  /** The message that began the turn at work (`atWork`); null when none is. */
  readonly turn: string | null
  /** The access the conversation said it ran at; null where it said none. */
  readonly access: AccessMode | null
}

/**
 * A consequential call, as the source recorded it: what was asked, of which
 * session, by whom and when — on record as the source begins to carry it out,
 * before the effect, and in that carry-out order — then the session as it
 * stood (`before`), what became of it and when, and, taken, the revision it
 * produced (`after`: the new summary's, the removal's, or, for a question
 * answered or declined, the transcript revision that dropped it — the
 * summary does not move). An answer keeps the ask as it was held and the
 * structured choices, including the answerer's own words. Those words stay
 * on this record: they are not copied onto the session, the transcript, or
 * a log. Calls asked in one
 * tick are carried out in the order they were asked, so a message and the
 * answer it overtakes stay in that order. This source refuses only by type;
 * it has no faults of its own to tell apart. Kept in memory only, for as long
 * as the source lives; entries are frozen and replaced, never changed.
 */
export interface AuditEntry {
  readonly sessionId: string
  /** `let-go`: a message sent while an approval waited moved the turn on without it. */
  readonly action:
    | "allow-once"
    | "allow-always"
    | "deny"
    | "pin"
    | "unpin"
    | "archive"
    | "rename"
    | "fork"
    | "delete"
    | "let-go"
    | "answer-question"
    | "decline-question"
    | "stop"
    | "set-access"
  /** The approval answered, or let go. */
  readonly approvalId?: string
  /** The question answered or declined, and the execution it belongs to. */
  readonly questionId?: string
  readonly executionId?: string
  /** The ask as held when it was answered, declined, or the answer was refused. */
  readonly question?: TranscriptQuestion
  /**
   * Structured choices for an answer, including own words. Absent when the
   * ask was declined. Own words are this controlled record's only copy.
   */
  readonly choices?: readonly QuestionChoice[]
  /** The access asked for, for a `set-access`. */
  readonly access?: AccessMode
  /** The title asked for, for a `rename`. */
  readonly title?: string
  /** The message that let an approval go, for a `let-go`. */
  readonly messageId?: string
  readonly initiator: Initiator
  readonly at: number
  /** The session when the source came to carry the call out, if it held it; absent until then. */
  readonly before?: AuditedState | null
  readonly outcome: "asked" | "taken" | { readonly refused: WorkspaceFailureReason }
  /**
   * The revision the call produced, once taken: the summary's, the removal's,
   * or the transcript's when a question was answered or declined.
   */
  readonly after?: number
  readonly settledAt?: number
}

export interface InMemorySource extends WorkspaceSource {
  /**
   * A message of the person's, written by the MCP App `app` (`ui/message`,
   * #390), as the gateway takes one: only while the session is idle — refused
   * `not-waiting` while its agent is at work or waits on the person — so it
   * never waits behind anything.
   */
  appMessage(sessionId: string, app: MessageApp, text: string): Promise<void>
  /** Every consequential call, in order — answers, pins, archives — refused ones included. */
  audit(): readonly AuditEntry[]
  /** Stops every scripted reply and refuses every later call; nothing is emitted afterwards. */
  dispose(): void
}

export function inMemorySource(
  schedule: Schedule,
  seed: ReturnType<typeof sampleWorkspace> = sampleWorkspace(schedule.now()),
): InMemorySource {
  // Archived and deleted ids: none is begun again, so no revision of one counts from 1 twice.
  const archived = new Set<string>()
  const sessions = new Map(seed.index.sessions.map((session) => [session.id, session]))
  const transcripts = new Map(seed.transcripts)
  const listeners = new Set<(update: WorkspaceUpdate) => void>()
  // Each session's running script, so a new message or `dispose` can stop it.
  const scripts = new Map<string, Set<() => void>>()
  let disposed = false
  const guard = () => {
    if (disposed) throw new WorkspaceSourceError("unavailable")
  }
  // A disposed source answers nothing: every later call is refused, and none schedules.
  const live = <T>(work: () => T): Promise<T> =>
    run(() => {
      guard()
      return work()
    })
  // On record as the call is carried out — a refused call, a disposed source's,
  // too — before its effect, then given the session as the source found it,
  // and what became of it. Appending here, not when the caller asked, keeps a
  // same-tick message ahead of the answer it overtakes: each call is one
  // microtask, and those run in the order the calls were made. Entries are
  // frozen and replaced, never changed.
  let entries: readonly AuditEntry[] = Object.freeze([])
  const stateOf = (sessionId: string): AuditedState | null => {
    const held = sessions.get(sessionId)
    if (!held) return null
    const transcript = transcripts.get(sessionId)
    const approval = transcript?.approvals[0]
    return Object.freeze({
      revision: held.revision,
      pinned: held.pinned,
      waitingOn: approval
        ? Object.freeze({ approvalId: approval.id, command: approval.command })
        : null,
      turn: atWork(transcript)
        ? ([...(transcript?.messages ?? [])]
            .reverse()
            .find((message) => message.role === "user")?.id ?? null)
        : null,
      access: transcript?.access?.mode ?? null,
    })
  }
  /** Runs a consequential call; `work` answers with the revision it produced. */
  const audited = (
    asked: Pick<
      AuditEntry,
      | "sessionId"
      | "action"
      | "approvalId"
      | "questionId"
      | "executionId"
      | "access"
      | "title"
      | "initiator"
    >,
    work: (record: (change: Partial<AuditEntry>) => void) => number,
  ): Promise<void> => {
    let entry: AuditEntry | undefined
    const replace = (change: Partial<AuditEntry>) => {
      if (!entry) throw new Error("an audit entry is replaced only after it is opened")
      const next: AuditEntry = Object.freeze({ ...entry, ...change })
      entries = Object.freeze(entries.map((each) => (each === entry ? next : each)))
      entry = next
    }
    return run(() => {
      const opened: AuditEntry = Object.freeze({
        ...asked,
        at: schedule.now(),
        outcome: "asked",
      })
      entry = opened
      entries = Object.freeze([...entries, opened])
      guard()
      replace({ before: stateOf(asked.sessionId) })
      return work(replace)
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

  const putTranscript = (change: Transcript): Transcript => {
    const transcript = {
      ...change,
      // Counted from what a read would have said, so no two contents share a revision.
      revision: transcriptOf(change.sessionId).revision + 1,
    }
    transcripts.set(transcript.sessionId, transcript)
    emit({ kind: "transcript", transcript })
    return transcript
  }

  const transcriptOf = (sessionId: string): Transcript =>
    // A session with no history yet: still the source's word, so revision 1.
    transcripts.get(sessionId) ?? { ...emptyTranscript(sessionId), revision: 1 }

  const stopScript = (sessionId: string) => {
    for (const cancel of scripts.get(sessionId) ?? []) cancel()
    scripts.delete(sessionId)
    // A reply cut off where it got to has ended with its turn: it is no
    // longer being written.
    const current = transcriptOf(sessionId)
    const writing = (message: Message) => "working" in message && message.working
    if (current.messages.some(writing))
      putTranscript({
        ...current,
        messages: current.messages.map((message) => {
          if (!writing(message)) return message
          const { working: _working, ...settled } = message as TurnMessage
          return settled
        }),
      })
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

  const agentMessage = (sessionId: string, parts: readonly Part[]): TurnMessage => ({
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
    // Being written until its last words are: then its turn has ended.
    const message: TurnMessage = {
      ...agentMessage(sessionId, [...steps, { kind: "text", text: "" }]),
      working: true,
    }
    const base = transcriptOf(sessionId)
    const write = (shown: number) => {
      const current = transcriptOf(sessionId)
      const { working: _working, ...settled } = message
      const written: Message = {
        ...(shown === words.length ? settled : message),
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

  const accept = (message: OutgoingMessage, app?: MessageApp) => {
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
      ...(app ? { app } : {}),
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
      // A new message moves the turn on: every approval still asked is let go.
      approvals: [],
      activity: { label: "Thinking", since: at },
      // A session begins at the access chosen for it; one begun keeps its own.
      ...(!existing && message.start?.access
        ? { access: { mode: message.start.access, modes: ["ask", "auto", "full"] } }
        : {}),
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
    const approval = transcript.approvals.find((asked) => asked.id === approvalId)
    if (!approval) throw new WorkspaceSourceError("not-waiting")
    return { transcript, approval }
  }

  const waitingQuestion = (
    sessionId: string,
    executionId: string,
    questionId: string,
  ) => {
    known(sessionId)
    const transcript = transcriptOf(sessionId)
    const asked = transcript.questions?.some(
      (question) =>
        question.executionId === executionId && question.questionId === questionId,
    )
    if (!asked) throw new WorkspaceSourceError("not-waiting")
    return transcript
  }

  /** An option the review does not offer, or of another choice, is not supported. */
  const offered = (approval: Approval, choice: ApprovalChoice, optionId: string) => {
    const option = approval.options.find((each) => each.id === optionId)
    if (!option || option.choice !== choice)
      throw new WorkspaceSourceError("not-supported")
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
    appMessage: (sessionId, app, text) =>
      live(() => {
        const held = sessions.get(sessionId)
        if (!held || archived.has(sessionId))
          throw new WorkspaceSourceError("unknown-session")
        if (held.status !== "idle") throw new WorkspaceSourceError("not-waiting")
        accept(
          {
            sessionId,
            initiator: "person",
            messageId: `app-${held.revision}-${schedule.now()}`,
            text,
            model: held.model,
          },
          app,
        )
      }),
    approve: (
      sessionId: string,
      approvalId: string,
      scope: ApprovalScope,
      initiator: Initiator,
      optionId: string,
    ) =>
      audited(
        {
          sessionId,
          approvalId,
          action: scope === "always" ? "allow-always" : "allow-once",
          initiator,
        },
        () => {
          const { transcript, approval } = waiting(sessionId, approvalId)
          offered(approval, scope === "always" ? "always" : "once", optionId)
          const command = approval.command
          putSession({
            ...known(sessionId),
            status: "running",
            now: runningNow(command),
            updatedAt: schedule.now(),
          })
          putTranscript({
            ...transcript,
            approvals: transcript.approvals.filter((asked) => asked.id !== approvalId),
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
    answerQuestion: (sessionId, executionId, questionId, choices, initiator) =>
      audited(
        {
          sessionId,
          action: choices === null ? "decline-question" : "answer-question",
          executionId,
          questionId,
          initiator,
        },
        (record) => {
          const transcript = waitingQuestion(sessionId, executionId, questionId)
          const held = transcript.questions?.find(
            (question) =>
              question.executionId === executionId && question.questionId === questionId,
          )
          if (!held) throw new WorkspaceSourceError("not-waiting")
          record({ question: Object.freeze(structuredClone(held)) })
          if (choices !== null) {
            let accepted: readonly QuestionChoice[]
            try {
              accepted = acceptedQuestionChoices(held, choices)
            } catch (error) {
              record({
                choices: Object.freeze(
                  choices.map((choice) =>
                    Object.freeze({
                      key: choice.key,
                      values: Object.freeze([...choice.values]),
                      ...(choice.ownWords === undefined
                        ? {}
                        : { ownWords: choice.ownWords }),
                    }),
                  ),
                ),
              })
              throw error
            }
            record({ choices: accepted })
          }
          return putTranscript({
            ...transcript,
            questions: (transcript.questions ?? []).filter(
              (question) =>
                question.executionId !== executionId ||
                question.questionId !== questionId,
            ),
          }).revision
        },
      ),
    deny: (sessionId, approvalId, initiator, optionId) =>
      audited({ sessionId, approvalId, action: "deny", initiator }, () => {
        const { transcript, approval } = waiting(sessionId, approvalId)
        offered(approval, "deny", optionId)
        const reply = deniedReply(approval.command)
        putTranscript({
          ...transcript,
          approvals: transcript.approvals.filter((asked) => asked.id !== approvalId),
          activity: null,
          messages: [...transcript.messages, agentMessage(sessionId, reply.parts)],
        })
        settle(sessionId, reply.preview)
        return known(sessionId).revision
      }),
    // The turn ends where it got to: its script stops, what it waited on is let go.
    stop: (sessionId, initiator) =>
      audited({ sessionId, action: "stop", initiator }, () => {
        known(sessionId)
        if (!atWork(transcriptOf(sessionId)))
          throw new WorkspaceSourceError("not-waiting")
        stopScript(sessionId)
        putTranscript({
          ...transcriptOf(sessionId),
          activity: null,
          approvals: [],
          questions: [],
        })
        return putSession({
          ...known(sessionId),
          status: "idle",
          now: undefined,
          updatedAt: schedule.now(),
        }).revision
      }),
    // As the gateway: changed only while no turn runs, and said by the conversation.
    setModel: () => Promise.reject(new WorkspaceModelError("not-supported", false)),
    setAccess: (sessionId, mode, initiator) =>
      audited({ sessionId, action: "set-access", access: mode, initiator }, () => {
        known(sessionId)
        const transcript = transcriptOf(sessionId)
        if (atWork(transcript)) throw new WorkspaceSourceError("unavailable")
        return putTranscript({
          ...transcript,
          access: { mode, modes: ["ask", "auto", "full"] },
        }).revision
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
    // As the gateway keeps a typed title; nothing left of it changes nothing.
    rename: (sessionId, typed, initiator) =>
      audited({ sessionId, action: "rename", title: typed, initiator }, () => {
        const title = conversationTitle(typed)
        const session = known(sessionId)
        return title === null || title === session.title
          ? session.revision
          : putSession({ ...session, title }).revision
      }),
    // As the gateway: a new session listed as this one is, whose transcript
    // is the turns copied and the line saying so, and that has said nothing
    // of its own yet.
    fork: (sessionId, initiator) => {
      const forked = crypto.randomUUID()
      return audited({ sessionId, action: "fork", initiator }, () => {
        const session = known(sessionId)
        const copied = transcriptOf(sessionId).messages.flatMap((message) =>
          message.role === "user" || message.role === "agent"
            ? [
                {
                  id: `/handoff:${forked}/${message.id}`,
                  role: message.role,
                  at: message.at,
                  parts: message.parts.filter((part) => part.kind === "text"),
                },
              ]
            : [],
        )
        putTranscript({
          ...emptyTranscript(forked),
          messages: [
            ...copied,
            {
              id: `/handoff:${forked}`,
              role: "handoff",
              at: schedule.now(),
              parts: [],
              handoff: { kind: "fork", omittedTurns: 0 },
            },
          ],
        })
        putSession({
          ...session,
          id: forked,
          status: "idle",
          now: undefined,
          pinned: false,
          unread: false,
          startedAt: schedule.now(),
          updatedAt: schedule.now(),
        })
        return session.revision
      }).then(() => forked)
    },
    // As the gateway: nothing of it is kept, and its id is never begun again.
    delete: (sessionId, initiator) =>
      audited({ sessionId, action: "delete", initiator }, () => {
        const revision = known(sessionId).revision + 1
        stopScript(sessionId)
        sessions.delete(sessionId)
        transcripts.delete(sessionId)
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
