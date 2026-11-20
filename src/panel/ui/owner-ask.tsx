import * as React from "react"
import { AccessRequestHost, OwnerAskCard, type OwnerAskView } from "../../conversation"

/**
 * The decision an agent asked the person in the conversation on screen,
 * above the composer, while the list says it is still asked. A tapped choice
 * is sent as the person's own message, as if typed; the list then says
 * nothing is asked and the card leaves. A choice not sent offers them all
 * again.
 *
 * An agent's request for more access is the red card, whose round is
 * `AccessRequestHost`'s, the same as the desktop window's: Approve raises
 * the conversation's access by the access picker's own command
 * (`raiseAccess`) and answers once the conversation's view says it runs at
 * it, every fact here as the view states it.
 */
export function PanelOwnerAsk({
  chat,
  ask,
  submit,
  raiseAccess,
  access,
  underway,
  revision,
  working,
}: {
  /** The tab's conversation: its Approve is kept under it while tabs change. */
  chat: string
  ask: OwnerAskView | undefined
  /** Sends text as the person's message; resolves whether the gateway took it. */
  submit: (text: string) => Promise<boolean>
  /** Raises the conversation's access, and says what became of it. */
  raiseAccess: (mode: "auto" | "full") => Promise<"taken" | "changing" | "refused">
  /**
   * The access the conversation runs at, as its view last said it; absent
   * while a change is on its way and not yet in effect.
   */
  access: "ask" | "auto" | "full" | undefined
  /** Where a change on its way is headed, as the view says, if one is. */
  underway?: "ask" | "auto" | "full" | undefined
  /** The view's revision: a different one is a newer view. */
  revision?: string | undefined
  /** The agent is at work, so access cannot change yet. */
  working: boolean
}) {
  const [sending, setSending] = React.useState<{ askId: string; label: string } | null>(
    null,
  )
  const [failure, setFailure] = React.useState<string | null>(null)
  const askId = ask?.askId
  React.useEffect(() => {
    setSending(null)
    setFailure(null)
  }, [askId])
  return (
    <>
      <AccessRequestHost
        chat={chat}
        ask={ask}
        runsAt={access}
        underway={underway}
        working={working}
        revision={revision}
        raise={raiseAccess}
        answer={(text) => submit(text).catch(() => false)}
      />
      {ask && !ask.access ? (
        <OwnerAskCard
          ask={ask}
          sending={sending?.askId === ask.askId ? sending.label : null}
          failure={failure}
          onChoose={(label) => {
            setSending({ askId: ask.askId, label })
            setFailure(null)
            const failed = () => {
              setSending(null)
              setFailure("This choice was not sent. Pick it again, or write your answer.")
            }
            submit(label).then((taken) => taken || failed(), failed)
          }}
        />
      ) : null}
    </>
  )
}
