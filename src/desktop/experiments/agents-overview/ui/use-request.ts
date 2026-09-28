import { useEffect, useState } from "react"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../../workspace"
import type { WorkspaceFailureReason } from "../../../workspace/model/failure"
import type { Transcript } from "../../../workspace/model/transcript"
import {
  readConversation,
  selectHeldConversation,
  type ConversationRead,
} from "../adapters/workspace-bridge"
import { newer, requestOf, type Request } from "../model/request"

/**
 * A session's conversation for the overview: read afresh whenever its
 * summary changes — the source counts a new ask, a finished turn, in the
 * summary too — and set beside what the window already holds of it, which
 * the source's stream keeps current; the newer wins (`newer`). Nothing is
 * read while `revision` is undefined.
 */
export function useConversation(
  sessionId: string,
  revision: number | undefined,
): { transcript?: Transcript; failure?: WorkspaceFailureReason } {
  const dispatch = useWorkspaceDispatch()
  const held = useWorkspaceSelector((state) => selectHeldConversation(state, sessionId))
  const [read, setRead] = useState<{
    sessionId: string
    result: ConversationRead
  } | null>(null)
  useEffect(() => {
    if (revision === undefined) return
    let current = true
    void dispatch(readConversation(sessionId)).then((result) => {
      if (current) setRead({ sessionId, result })
    })
    return () => {
      current = false
    }
  }, [dispatch, sessionId, revision])
  const mine = read?.sessionId === sessionId ? read.result : undefined
  const transcript = newer(held, mine?.kind === "read" ? mine.transcript : undefined)
  return transcript
    ? { transcript }
    : { failure: mine?.kind === "failed" ? mine.reason : undefined }
}

/** What a waiting session asks: its approval, a question, or not read yet. */
export function useRequest(sessionId: string, revision: number | undefined): Request {
  const { transcript, failure } = useConversation(sessionId, revision)
  if (!transcript && failure) return { kind: "unreadable", reason: failure }
  return requestOf(transcript)
}
