/**
 * The window's voice call, for whichever pane shows it: the controller
 * composition made (`main.tsx`), and its snapshot as React state. Without a
 * provider — the sample preview, a test — there is no call to start, and
 * the composer offers no voice button.
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react"
import { Button } from "@nessa-ui/react/button"
import { Dialog, DialogClose } from "@nessa-ui/react/dialog"
import type { VoiceController, VoiceSnapshot } from "../application/voice-controller"

interface Voice {
  readonly controller: VoiceController
  /** Starts a call about a chat, asking first when it would end another. */
  readonly start: (sessionId: string) => void
}

const VoiceContext = createContext<Voice | undefined>(undefined)

/** Whether a call is open and talks to another chat than `sessionId`. */
export function callElsewhere(snapshot: VoiceSnapshot, sessionId: string): boolean {
  switch (snapshot.phase) {
    case "connecting":
    case "listening":
    case "working":
    case "speaking":
      return snapshot.sessionId !== sessionId
    case "idle":
    case "failed":
    case "dropped":
      return false
  }
}

const noSubscription = () => () => {}
const noCall = () => undefined
const notShown = () => false

/**
 * The window has one call. Talk in a chat while a call talks to another
 * would end that call, so the person is asked first, once for the window:
 * the question names the chat whose call ends (`title`, by its session).
 *
 * A call goes on while no pane shows its chat, so one that fails or drops
 * then has no pane to say so in. The window says it instead, with the way
 * back to the chat (`showChat`), where the chat's own notice offers the fix.
 */
export function VoiceProvider({
  controller,
  title,
  showChat,
  children,
}: {
  controller: VoiceController | undefined
  title?: (sessionId: string) => string | null
  showChat?: (sessionId: string) => void
  children: ReactNode
}) {
  const subscribe = controller
    ? (listener: () => void) => controller.subscribe(listener)
    : noSubscription
  const snapshot = useSyncExternalStore(
    subscribe,
    controller ? () => controller.snapshot() : noCall,
  )
  const chatShown = useSyncExternalStore(
    subscribe,
    controller ? () => controller.chatShown() : notShown,
  )
  // The chat Talk was pressed in while a call talks to another.
  const [asked, setAsked] = useState<string | null>(null)
  const start = useCallback(
    (sessionId: string) => {
      if (!controller) return
      if (callElsewhere(controller.snapshot(), sessionId)) setAsked(sessionId)
      else void controller.start(sessionId)
    },
    [controller],
  )
  const voice = useMemo(
    () => (controller ? { controller, start } : undefined),
    [controller, start],
  )
  // The question stands only while there is a call elsewhere to end: one
  // that ended meanwhile (on the pill, or dropped) takes the question with it.
  const moving =
    asked !== null && snapshot !== undefined && callElsewhere(snapshot, asked)
      ? { to: asked, from: snapshot.sessionId }
      : null
  const stale = asked !== null && moving === null
  useEffect(() => {
    if (stale) setAsked(null)
  }, [stale])
  const named = (sessionId: string | null) =>
    sessionId === null ? "" : (title?.(sessionId)?.trim() ?? "")
  const ended =
    snapshot !== undefined &&
    (snapshot.phase === "failed" || snapshot.phase === "dropped") &&
    snapshot.sessionId !== null &&
    !chatShown
      ? { sessionId: snapshot.sessionId, notice: snapshot.notice ?? "The call ended." }
      : null
  return (
    <VoiceContext.Provider value={voice}>
      {children}
      {controller && moving ? (
        <Dialog
          open
          onOpenChange={(next) => {
            if (!next) setAsked(null)
          }}
          size="sm"
          title="Move the call here?"
          description={
            named(moving.from)
              ? `Nessa holds one call at a time. Your call in “${named(moving.from)}” will end.`
              : "Nessa holds one call at a time. Your other call will end."
          }
          showCloseButton={false}
          data-voice-move=""
          footer={
            <>
              <DialogClose asChild>
                <Button variant="outline">Cancel</Button>
              </DialogClose>
              <Button
                data-voice-move-action="confirm"
                onClick={() => {
                  setAsked(null)
                  void controller.start(moving.to)
                }}
              >
                Talk here
              </Button>
            </>
          }
        />
      ) : controller && ended ? (
        <Dialog
          open
          onOpenChange={(next) => {
            if (!next) controller.dismiss()
          }}
          size="sm"
          title={
            named(ended.sessionId)
              ? `The call in “${named(ended.sessionId)}” ended`
              : "The call ended"
          }
          description={ended.notice}
          showCloseButton={false}
          data-voice-ended=""
          footer={
            <>
              {showChat ? (
                <Button
                  variant="outline"
                  data-voice-ended-action="open"
                  onClick={() => showChat(ended.sessionId)}
                >
                  Open chat
                </Button>
              ) : null}
              <DialogClose asChild>
                <Button data-voice-ended-action="ok">OK</Button>
              </DialogClose>
            </>
          }
        />
      ) : null}
    </VoiceContext.Provider>
  )
}

export function useVoiceController(): VoiceController | undefined {
  return useContext(VoiceContext)?.controller
}

/** Starts a call about a chat, asking first when it would end another; undefined without a controller. */
export function useVoiceStart(): ((sessionId: string) => void) | undefined {
  return useContext(VoiceContext)?.start
}

/** The call as it stands; undefined without a controller. */
export function useVoiceSnapshot(): VoiceSnapshot | undefined {
  const controller = useVoiceController()
  return useSyncExternalStore(
    controller ? (listener) => controller.subscribe(listener) : noSubscription,
    controller ? () => controller.snapshot() : noCall,
  )
}
