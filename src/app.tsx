import * as React from "react"
import { Plus, Square } from "lucide-react"
import { ChatComposerAction, ChatComposerInput } from "@nessa-ui/react/chat-composer"
import { ChatTabs, type ChatTabItem } from "@nessa-ui/react/chat-tabs"
import { PillComposer, PillComposerRow } from "@nessa-ui/react/pill-composer"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"

import { AGENT_HUES } from "./agent-identity"
import {
  hostKind,
  startResizeFromLeftEdge,
} from "./host-window"
import { Transcript } from "./transcript"
import { useColorScheme } from "./use-color-scheme"
import { useConversation } from "./use-conversation"
import { useEdgeReveal } from "./use-edge-reveal"
import { useHostPanel } from "./use-host-panel"
import { useSurface } from "./use-surface"
import { WaveformIcon } from "./waveform-icon"

export function App() {
  const scheme = useColorScheme()
  const ground = scheme === "dark" ? "ink" : "paper"
  const [surface, toggleSurface] = useSurface()
  const edge = useEdgeReveal()
  const strip = useConversation()
  const composerRef = React.useRef<HTMLTextAreaElement>(null)
  useHostPanel(surface, toggleSurface, composerRef)

  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    strip.submit()
  }

  const generating = strip.active.phase !== "idle"

  const tabs: ChatTabItem[] = strip.conversations.map((item) => ({
    id: item.id,
    title: item.title,
    closeable: true,
    // The avatars are still, so the dot is the only thing saying which thread
    // is mid-reply.
    loading: item.phase !== "idle",
    icon: (
      <RandomAvatar
        seed={item.id}
        hues={AGENT_HUES}
        ground={ground}
        className="size-4 rounded-full"
      />
    ),
  }))

  return (
    <div className="nessa-stage">
      <div
        ref={edge.panelRef}
        data-nessa-root
        data-surface={surface}
        data-host={hostKind}
        className="nessa-panel relative flex min-h-0 flex-col overflow-hidden rounded-[18px] border"
        onPointerMove={edge.onPointerMove}
        onPointerLeave={edge.onPointerLeave}
      >
        {/* Lights the stretch of border nearest the pointer. Inert on
            purpose: it covers the whole panel, and anything interactive here
            would steal every click in the transcript. */}
        <div
          ref={edge.glowRef}
          aria-hidden="true"
          className="nessa-edge-reveal pointer-events-none"
        />
        {/* A grab handle for the left edge. macOS claims the window frame
            before the webview sees it, so this only ever fires just inside the
            system's own grab zone — the glow is pinned from window size
            events rather than from this gesture. On Linux the handle *is* the
            resize, so it is wide enough to grab and pointerdown here both
            starts the drag and pins the glow. */}
        <div
          role="presentation"
          onPointerDown={(event) => {
            // Pointer capture on Linux holds the button on the webview, so
            // GTK's `begin_resize_drag` does not see it and the west resize
            // never starts. The system's 5px inset still works; this path is
            // the rest of the handle.
            if (hostKind !== "linux") {
              event.currentTarget.setPointerCapture(event.pointerId)
            }
            edge.holdResize()
            void startResizeFromLeftEdge()
          }}
          onPointerUp={edge.releaseResize}
          className={
            hostKind === "linux"
              ? "absolute inset-y-0 left-0 z-10 w-3 cursor-ew-resize"
              : "absolute inset-y-0 left-0 z-10 w-1.5 cursor-ew-resize"
          }
        />

        {/* The strip is the titlebar too: the gaps around the tabs drag the
            window, while the tabs themselves stay clickable. */}
        <div
          data-tauri-drag-region
          className="nessa-chrome shrink-0 px-1.5 pt-2 pb-1"
        >
          <ChatTabs
            label="Conversations"
            tabs={tabs}
            value={strip.active.id}
            onValueChange={strip.setActiveId}
            onClose={strip.closeConversation}
            onNew={() => {
              strip.openConversation()
              composerRef.current?.focus()
            }}
            newTabLabel="New conversation"
          />
        </div>

        <Transcript conversation={strip.active} ground={ground} />

        <div className="nessa-chrome shrink-0 px-2.5 pb-2.5">
          <PillComposer generating={generating} onSubmit={submit}>
            <PillComposerRow>
              <ChatComposerAction aria-label="Add attachment" title="Add attachment">
                <Plus aria-hidden="true" />
              </ChatComposerAction>
              <ChatComposerInput
                ref={composerRef}
                value={strip.active.draft}
                onChange={(event) => strip.setDraft(event.target.value)}
                placeholder="Ask me anything"
                className="self-center"
                autoFocus
              />
              {/* Enter is the send affordance, as the pill intends, so the
                  trailing slot carries voice instead — and hands over to the
                  way to stop a reply while one is arriving. Voice is inert
                  until there is a runtime to transcribe into. */}
              {generating ? (
                <ChatComposerAction
                  aria-label="Stop generating"
                  title="Stop generating"
                  onClick={strip.stopGenerating}
                >
                  <Square aria-hidden="true" className="fill-current" />
                </ChatComposerAction>
              ) : (
                <ChatComposerAction
                  aria-label="Start voice input"
                  title="Start voice input"
                >
                  <WaveformIcon className="size-[18px]" />
                </ChatComposerAction>
              )}
            </PillComposerRow>
          </PillComposer>
        </div>
      </div>

      {strip.clocks}
    </div>
  )
}
