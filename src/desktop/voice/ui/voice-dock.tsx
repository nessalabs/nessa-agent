/**
 * What a conversation's pane shows while a call talks to it: the call's tab,
 * tucked into the top of the composer, and its light behind the composer
 * (`voice-glow.tsx`). The composer itself does not change during a call.
 *
 * The tab is one width whatever is said, and one height unless the person
 * opens it: a header saying where the call stands, with the call's switches,
 * and under it the last few lines said, the newest word by word. Opened, the
 * same tab is taller and holds the whole call to scroll back through. A call
 * that failed or dropped keeps only the header: what stopped it, the one
 * thing that fixes it there, and a close.
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { Button } from "@nessa-ui/react/button"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"
import { AGENT_HUES, AGENT_ICON_TONE, AGENT_SEED } from "../../../conversation/model"
import { IconButton } from "../../ui/icon-button"
import { VoiceKeyButton } from "../../settings/ui/voice-key-row"
import { useVaultUnlock } from "../../../vault"
import type { VoiceLine, VoiceSnapshot } from "../application/voice-controller"
import { useVoiceController, useVoiceSnapshot, useVoiceStart } from "./voice-context"
import { VoiceGlow } from "./voice-glow"
import "./voice.css"

/** How many lines the closed tab shows: the newest, older ones fading above. */
export const SHOWN_LINES = 3

/**
 * Where the call stands, in the tab's header: a word or two, or for a call
 * that stopped, why. `agentName` names who works on what the voice sent.
 */
export function voiceStatus(snapshot: VoiceSnapshot, agentName?: string): string {
  switch (snapshot.phase) {
    case "connecting":
      return "Connecting…"
    case "listening":
      return snapshot.microphoneOn ? "Listening" : "Microphone off"
    case "working":
      return agentName ? `${agentName} is working…` : "Working…"
    case "speaking":
      return "Speaking"
    case "paused":
      return "Paused"
    case "failed":
    case "dropped":
      return snapshot.notice ?? ""
    case "idle":
      return ""
  }
}

/** A line in the tab: one written down, or words still being said or heard. */
export interface ShownLine extends VoiceLine {
  readonly live: boolean
}

/**
 * The call's lines, oldest first: what was written down, then what the voice
 * is saying and what the person is saying, as they come.
 */
export function voiceLines(snapshot: VoiceSnapshot): ShownLine[] {
  const lines: ShownLine[] = snapshot.transcript.map((line) => ({ ...line, live: false }))
  const saying = snapshot.saying.trim()
  if (snapshot.phase === "speaking" && saying)
    lines.push({ who: "voice", text: saying, live: true })
  const hearing = snapshot.hearing.trim()
  if (hearing && snapshot.phase !== "connecting")
    lines.push({ who: "person", text: hearing, live: true })
  return lines
}

export function VoiceDock({
  sessionId,
  agent,
  agentName,
}: {
  sessionId: string
  /** The agent the chat runs, whose colour the call's light takes. */
  agent?: string
  /** That agent's name, for "Claude is working…". */
  agentName?: string
}) {
  const controller = useVoiceController()
  const snapshot = useVoiceSnapshot()
  const start = useVoiceStart()
  // The pane says it shows this conversation: a call talking to it has its
  // switches here, and on the pill once no pane shows it.
  useEffect(() => controller?.show(sessionId), [controller, sessionId])
  const unlock = useVaultUnlock()
  const levels = useCallback(
    () => controller?.levels() ?? { input: 0, output: 0 },
    [controller],
  )
  const [open, setOpen] = useState(false)
  const mine = snapshot?.sessionId === sessionId ? snapshot : undefined
  if (!controller || !mine || mine.phase === "idle") return null
  const ended = mine.phase === "failed" || mine.phase === "dropped"
  const status = voiceStatus(mine, agentName)
  return (
    <div className="voice-dock" data-phase={mine.phase} data-agent={agent ?? "none"}>
      {ended ? null : (
        <VoiceGlow phase={mine.phase} microphoneOn={mine.microphoneOn} levels={levels} />
      )}
      <section
        className="voice-tab"
        aria-label="Voice call"
        data-open={open && !ended ? "" : undefined}
        data-ended={ended ? "" : undefined}
      >
        <div className="voice-tab-head">
          <RandomAvatar
            seed={AGENT_SEED}
            hues={AGENT_HUES}
            tone={AGENT_ICON_TONE}
            ground="paper"
            className="voice-tab-avatar"
            aria-hidden="true"
          />
          <span
            className="voice-tab-status"
            role="status"
            data-notice-fix={ended ? (mine.fix ?? "none") : undefined}
          >
            {status}
          </span>
          {ended ? (
            <>
              {mine.fix === "unlock" ? (
                <Button
                  variant="tinted"
                  shape="pill"
                  size="26"
                  className="voice-tab-fix"
                  data-voice-fix="unlock"
                  onClick={() => {
                    void unlock().then((unlocked) => {
                      // Unlocked, the call starts as the person first asked,
                      // or asks for the key if none is saved.
                      if (unlocked) start?.(sessionId)
                    })
                  }}
                >
                  Unlock
                </Button>
              ) : mine.fix === "key" ? (
                <VoiceKeyButton
                  className="voice-tab-fix"
                  onSaved={() => void controller.start(sessionId)}
                >
                  Add key
                </VoiceKeyButton>
              ) : mine.phase === "dropped" ? (
                <Button
                  variant="tinted"
                  shape="pill"
                  size="26"
                  className="voice-tab-fix"
                  data-voice-fix="again"
                  onClick={() => start?.(sessionId)}
                >
                  Call again
                </Button>
              ) : null}
              <IconButton
                icon="close"
                label="Dismiss"
                size="sm"
                shape="pill"
                tone="faint"
                className="voice-tab-close"
                data-voice-dismiss
                onClick={() => controller.dismiss()}
              />
            </>
          ) : (
            <>
              <IconButton
                icon={mine.microphoneOn ? "microphone" : "microphoneOff"}
                label={mine.microphoneOn ? "Mute microphone" : "Unmute microphone"}
                size="sm"
                shape="pill"
                className="voice-tab-switch"
                aria-pressed={!mine.microphoneOn}
                onClick={() => controller.setMicrophone(!mine.microphoneOn)}
              />
              <IconButton
                icon={mine.speakerOn ? "speaker" : "speakerOff"}
                label={mine.speakerOn ? "Mute voice" : "Unmute voice"}
                size="sm"
                shape="pill"
                className="voice-tab-switch"
                aria-pressed={!mine.speakerOn}
                onClick={() => controller.setSpeaker(!mine.speakerOn)}
              />
              <IconButton
                icon={mine.phase === "paused" ? "resume" : "pause"}
                label={mine.phase === "paused" ? "Resume call" : "Pause call"}
                size="sm"
                shape="pill"
                className="voice-tab-switch"
                aria-pressed={mine.phase === "paused"}
                onClick={() =>
                  mine.phase === "paused" ? void controller.resume() : controller.pause()
                }
              />
              <IconButton
                icon={open ? "restore" : "maximize"}
                label={open ? "Hide transcript" : "Show transcript"}
                size="sm"
                shape="pill"
                className="voice-tab-switch"
                aria-expanded={open}
                data-voice-transcript=""
                onClick={() => setOpen(!open)}
              />
              <IconButton
                icon="close"
                label="End call"
                size="sm"
                shape="pill"
                className="voice-tab-end"
                onClick={() => controller.end()}
              />
            </>
          )}
        </div>
        {ended ? null : (
          // Opening or closing starts it again, following the newest line.
          <VoiceWords key={open ? "open" : "closed"} snapshot={mine} open={open} />
        )}
      </section>
    </div>
  )
}

/**
 * The tab's lines. Closed, the newest few sit at its foot, the older fading
 * up and out; open, every line of the call, with who said it, to scroll.
 * Words still being said come in one by one: each word is its own element,
 * kept by its place in the line, so only a new word fades in.
 */
function VoiceWords({ snapshot, open }: { snapshot: VoiceSnapshot; open: boolean }) {
  const lines = voiceLines(snapshot)
  const shown = open ? lines : lines.slice(-SHOWN_LINES)
  const box = useRef<HTMLDivElement>(null)
  // Open, the newest line stays in view, unless the person scrolled up to read.
  const following = useRef(true)
  useLayoutEffect(() => {
    const element = box.current
    if (element && following.current) element.scrollTop = element.scrollHeight
  })
  return (
    <div
      ref={box}
      className="voice-tab-words"
      onScroll={(event) => {
        const element = event.currentTarget
        following.current =
          element.scrollHeight - element.scrollTop - element.clientHeight < 8
      }}
    >
      {shown.length === 0 ? (
        <p className="voice-tab-hint">{snapshot.hint ?? "Say something to start."}</p>
      ) : (
        shown.map((line, at) => {
          const age = shown.length - 1 - at
          return (
            <p
              // Lines are only ever added at the end, so place names a line.
              key={lines.length - shown.length + at}
              className="voice-tab-line"
              data-who={line.who}
              data-age={open ? undefined : Math.min(age, 2)}
            >
              <span className="voice-tab-who">
                {line.who === "person" ? "You" : "Nessa"}
              </span>
              <span className="voice-tab-said">
                {line.live
                  ? line.text.split(/(\s+)/).map((word, place) => (
                      <span key={place} className="voice-tab-word">
                        {word}
                      </span>
                    ))
                  : line.text}
              </span>
            </p>
          )
        })
      )}
    </div>
  )
}
