import * as React from "react"
import { Plus, Square } from "lucide-react"
import { ChatComposerAction } from "@nessa-ui/react/chat-composer"
import { ChatTabs, type ChatTabItem } from "@nessa-ui/react/chat-tabs"
import { PillComposer, PillComposerRow } from "@nessa-ui/react/pill-composer"
import { ChatComposerMarkdownEditor } from "@nessa-ui/react/chat-composer-markdown-editor"
import {
  Sheet,
  SheetHandle,
  SheetHeader,
  SheetExpand,
  SheetTitle,
  SheetAction,
  SheetBody,
} from "@nessa-ui/react/sheet"
import { MessageMarkdown } from "@nessa-ui/react/message-markdown"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"

import { AGENT_HUES, Transcript, useConversation, toEditor } from "../../conversation"
import { host, startResizeFromLeftEdge, type CompositorKind } from "../../host"
import { useSession } from "../../session"
import { useColorScheme } from "../adapters/color-scheme"
import { useFlushOnTurn } from "../adapters/compositor-flush"
import { useEdgeReveal } from "../adapters/edge-reveal"
import { useHostPanel } from "../adapters/host-panel"
import { useSurface, type Surface } from "../adapters/surface"
import { useTabShortcuts } from "../adapters/use-tab-shortcuts"
import { useComposer } from "./use-composer"

import { WaveformIcon } from "./waveform-icon"

// Draft and stream updates must not reparse the unchanged pasted document.
const PastedMarkdown = React.memo(MessageMarkdown)

/**
 * The panel's shape.
 *
 * Clear removes the frame — no background, no visible border — but it keeps its
 * containing block and its border box, because that box is what the edge reveal
 * is masked to. Without them the lit edge has nothing to trace, and clear is the
 * surface that needs it most: there is no frame otherwise to say where the
 * window ends or where it can be grabbed.
 *
 * A layout compositor is the exception. WebKitGTK fills a positioned panel's
 * layer with opaque white around the rounded pill, so there the panel is
 * flattened and the reveal is given up with it (see styles.css).
 */
function panelClass(surface: Surface, compositor: CompositorKind): string {
  const base = "nessa-panel flex min-h-0 flex-col"
  if (surface !== "clear") {
    return `${base} relative overflow-hidden rounded-[18px] border`
  }
  return compositor === "layout"
    ? `${base} overflow-visible border-0`
    : `${base} relative overflow-visible rounded-[18px] border`
}

export function App() {
  const scheme = useColorScheme()
  const ground = scheme === "dark" ? "ink" : "paper"
  const [surface, toggleSurface] = useSurface()
  const edge = useEdgeReveal()
  const chat = useConversation()
  const session = useSession()
  const {
    composerRef,
    setComposerRef,
    viewedPaste,
    closePaste,
    openPaste,
    focusComposer,
    submit,
    changeContent,
    pressChip,
    pasteAttachment,
  } = useComposer(chat)
  const openTab = React.useEffectEvent(() => {
    closePaste()
    chat.openConversation()
    focusComposer()
  })
  const closeActiveTab = React.useEffectEvent(() => {
    closePaste()
    chat.closeConversation(chat.active.id)
  })
  const activateTab = React.useEffectEvent(
    (target: { index?: number; conversationId?: string }) => {
      if (target.conversationId) {
        const open = chat.conversations.some((item) => item.id === target.conversationId)
        if (open) {
          closePaste()
          chat.setActive(target.conversationId)
          return
        }
      }
      if (typeof target.index !== "number") return
      const next = chat.conversations[target.index]
      if (!next) return
      closePaste()
      chat.setActive(next.id)
    },
  )
  const moveActiveTab = React.useEffectEvent((direction: -1 | 1) => {
    closePaste()
    chat.moveActive(direction)
  })
  useHostPanel(surface, toggleSurface, composerRef)
  useTabShortcuts({ openTab, closeActiveTab, moveActiveTab, activateTab })
  useFlushOnTurn(
    host.flushOnTurn,
    `${chat.active.id}:${chat.active.phase}:${chat.active.turns.length}`,
  )

  const generating = chat.active.phase !== "idle"

  const tabs: ChatTabItem[] = chat.conversations.map((item) => ({
    id: item.id,
    title: item.title,
    closeable: true,
    icon: (
      <RandomAvatar
        seed={item.id}
        hues={AGENT_HUES}
        ground={ground}
        busy={item.phase !== "idle"}
        speed={2}
        className="size-5 rounded-full"
      />
    ),
  }))

  return (
    <div className="nessa-stage" data-host={host.kind}>
      {/* On the stage, not the panel: a positioned descendant of the panel
          makes WebKitGTK fill a layer from the panel origin with opaque
          white. The stage's bottom-right is the window, so window-size
          tokens pin this strip to the panel's left edge. */}
      <div
        role="presentation"
        onPointerDown={(event) => {
          // Pointer capture on Linux holds the button on the webview, so
          // GTK's `begin_resize_drag` does not see it and the west resize
          // never starts. The system's 5px inset still works; this path is
          // the rest of the handle.
          if (host.capturePointerOnWestHandle) {
            event.currentTarget.setPointerCapture(event.pointerId)
          }
          edge.holdResize()
          void startResizeFromLeftEdge()
        }}
        onPointerUp={edge.releaseResize}
        className={host.westHandleClass}
      />
      <div
        ref={edge.panelRef}
        data-nessa-root
        data-surface={surface}
        data-host={host.kind}
        data-frost={host.frost}
        data-compositor={host.compositor}
        className={panelClass(surface, host.compositor)}
        onPointerMove={edge.onPointerMove}
        onPointerLeave={edge.onPointerLeave}
      >
        {/* Lights the stretch of border nearest the pointer. Inert on
            purpose: it covers the whole panel, and anything interactive here
            would steal every click in the transcript. */}
        <div
          ref={edge.glowRef}
          aria-hidden="true"
          // Only a layout compositor gives the reveal up — see `panelClass`.
          // Everywhere else clear is the surface that needs it most.
          hidden={surface === "clear" && host.compositor === "layout"}
          className="nessa-edge-reveal pointer-events-none"
        />
        {/* The tabs bar is the titlebar too: the gaps around the tabs drag the
            window, while the tabs themselves stay clickable.
        
            `deep` rather than a bare attribute. Bare means Tauri only drags on
            a *direct* hit of this element (`el === composedPath[0]`), and the
            tab list is `flex-1`, so its scroller covers the bar and takes
            every press — there is no gap left to land on. `deep` drags from
            anywhere in the subtree, and Tauri still refuses over anything
            clickable, which is what keeps the tabs themselves tabs. */}
        <div
          data-tauri-drag-region="deep"
          className="nessa-chrome shrink-0 px-2 pt-2 pb-1"
        >
          <ChatTabs
            label="Conversations"
            tabs={tabs}
            value={chat.active.id}
            onValueChange={(id) => {
              closePaste()
              chat.setActive(id)
            }}
            onClose={(id) => {
              closePaste()
              chat.closeConversation(id)
            }}
            onNew={() => {
              closePaste()
              chat.openConversation()
              focusComposer()
            }}
            newTabLabel="New conversation"
          />
        </div>

        {/* Keyed so a layout compositor drops the previous conversation's
            tiles instead of leaving them over the wallpaper. */}
        <div className="nessa-transcript-region relative flex min-h-0 flex-1 flex-col">
          <Transcript
            key={chat.active.id}
            conversation={chat.active}
            ground={ground}
            animateMount={host.animateMount}
            streamText={host.streamText}
            emptyState={host.emptyState}
            statusLabel={session.statusLabel}
            onOpenPaste={openPaste}
          />

          {viewedPaste?.conversationId === chat.active.id && (
            <Sheet
              className="nessa-pasted-viewer"
              label="Pasted text"
              onClose={closePaste}
              onReturnFocus={focusComposer}
            >
              <SheetHandle />
              <SheetHeader>
                <SheetExpand />
                <SheetTitle>Pasted text</SheetTitle>
                <SheetAction>Done</SheetAction>
              </SheetHeader>
              <SheetBody>
                <PastedMarkdown className="select-text min-w-0 [&_p]:whitespace-pre-wrap">
                  {viewedPaste.text}
                </PastedMarkdown>
              </SheetBody>
            </Sheet>
          )}
        </div>
        <div className="nessa-composer">
          <PillComposer
            key={chat.active.id}
            expandable={viewedPaste === null}

            generating={generating}
            onSubmit={submit}
          >
            <PillComposerRow>
              <ChatComposerAction aria-label="Add attachment" title="Add attachment">
                <Plus aria-hidden="true" />
              </ChatComposerAction>
              <ChatComposerMarkdownEditor
                key={`${chat.active.id}:${chat.active.turns.filter((turn) => turn.from === "user").length}`}
                ref={setComposerRef}
                defaultContent={toEditor(chat.active.draft)}
                onContentChange={changeContent}
                onChipPress={pressChip}
                pasteAttachmentMinLength={500}
                onPasteAttachment={pasteAttachment}
                placeholder="Ask me anything"
                aria-label="Message"
                maxHeight={240}
              />
              {/* Enter sends; Shift+Enter starts a new Markdown block.
                  Voice stays visible while typing and remains inert until wired. */}
              {generating ? (
                <ChatComposerAction
                  aria-label="Stop generating"
                  title="Stop generating"
                  onClick={chat.stopGenerating}
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
    </div>
  )
}
