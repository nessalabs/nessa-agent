import { ComposerDeliveryMode } from "@nessa-ui/react/composer-queue"
import type { AttachmentResources } from "../adapters/attachment-resources"
import * as React from "react"
import { Square, X } from "lucide-react"
import {
  ChatComposerAction,
  ChatComposerAttachments,
} from "@nessa-ui/react/chat-composer"
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

import {
  ConversationTabMenu,
  ConversationDetails,
  AGENT_HUES,
  ConversationQueue,
  ConversationNotification,
  Transcript,
  useConversation,
  toEditor,
} from "../../conversation"
import { host, startResizeFromLeftEdge, type CompositorKind } from "../../host"
import { useSession } from "../../session"
import { useColorScheme } from "../adapters/color-scheme"
import { useFlushOnTurn } from "../adapters/compositor-flush"
import { useEdgeReveal } from "../adapters/edge-reveal"
import { useHostPanel } from "../adapters/host-panel"
import { useSurface, type Surface } from "../adapters/surface"
import { useTabShortcuts } from "../adapters/use-tab-shortcuts"
import { useComposer } from "./use-composer"
import { useFileAttachments } from "./use-file-attachments"
import { useFolderDrop } from "./use-folder-drop"
import { useContentDrop } from "./use-content-drop"
import { FileDropZone } from "@nessa-ui/react/file-drop-zone"
import { ChatAttachmentTile } from "@nessa-ui/react/chat-bubbles"

import { AddAttachmentMenu } from "./add-attachment-menu"
import { AttachmentIcon } from "./attachment-icon"
import { WaveformIcon } from "./waveform-icon"

// Draft and stream updates must not reparse the unchanged pasted document.
const AttachmentPreview = React.lazy(() => import("./attachment-preview"))
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

export function App({
  attachmentResources,
  onSignOut,
  sessionError,
}: {
  attachmentResources: AttachmentResources
  onSignOut?: () => void
  sessionError?: string
}) {
  const scheme = useColorScheme()
  const ground = scheme === "dark" ? "ink" : "paper"
  const [surface, toggleSurface] = useSurface()
  const edge = useEdgeReveal()
  const chat = useConversation()
  const [tabDetails, setTabDetails] = React.useState<{
    id: string
    rename: boolean
  } | null>(null)
  const detailsConversation = chat.conversations.find(
    (item) => item.id === tabDetails?.id,
  )
  const attachments = useFileAttachments(chat, attachmentResources)
  const folderDrop = useFolderDrop(
    chat.active.id,
    attachments.addFiles,
    attachments.setError,
  )
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
  } = useComposer(chat, (id) => attachments.isPending(id) || folderDrop.isPending(id))
  const contentDrop = useContentDrop({
    addFolderEntries: folderDrop.addFolderEntries,
    addImageUrl: attachments.addImageUrl,
    focusComposer,
    pasteAttachment,
  })
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
      <FileDropZone
        asChild
        onFiles={attachments.addFiles}
        onRejectedFiles={() =>
          attachments.setError(
            "Some dropped files could not be attached. Try selecting them with +.",
          )
        }
        maxFiles={20}
        maxSize={20 * 1024 * 1024}
      >
        <div
          ref={edge.panelRef}
          data-nessa-root
          data-content-dragging={contentDrop.dragging || undefined}
          {...contentDrop.handlers}
          data-surface={surface}
          data-host={host.kind}
          data-frost={host.frost}
          data-compositor={host.compositor}
          className={panelClass(surface, host.compositor)}
          onPointerMove={edge.onPointerMove}
          onPointerLeave={edge.onPointerLeave}
        >
          <input
            ref={attachments.inputRef}
            type="file"
            multiple
            className="hidden"
            aria-label="Choose attachments"
            onChange={(event) => {
              const files = Array.from(event.currentTarget.files ?? [])
              event.currentTarget.value = ""
              void attachments.addFiles(files)
            }}
          />
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
              wrapTab={(tab, node) => (
                <ConversationTabMenu
                  key={tab.id}
                  onDetails={() => {
                    chat.setActive(tab.id)
                    setTabDetails({ id: tab.id, rename: false })
                  }}
                  onRename={() => setTabDetails({ id: tab.id, rename: true })}
                >
                  {node}
                </ConversationTabMenu>
              )}
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
              gatewayAvailable={chat.gatewayAvailable}
              onOpenPaste={openPaste}
            />

            {attachments.viewed && (
              <Sheet
                className="nessa-detail-sheet"
                label={attachments.viewed.name}
                onClose={attachments.close}
                onReturnFocus={focusComposer}
              >
                <SheetHandle />
                <SheetHeader>
                  <SheetExpand />
                  <SheetTitle>{attachments.viewed.name}</SheetTitle>
                  <SheetAction>Done</SheetAction>
                </SheetHeader>
                <SheetBody>
                  <React.Suspense fallback={<p role="status">Loading preview…</p>}>
                    <AttachmentPreview file={attachments.viewed} />
                  </React.Suspense>
                </SheetBody>
              </Sheet>
            )}
            {!attachments.viewed && viewedPaste?.conversationId === chat.active.id && (
              <Sheet
                className="nessa-detail-sheet"
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
          {tabDetails && detailsConversation && (
            <ConversationDetails
              key={`${tabDetails.id}:${tabDetails.rename}`}
              conversation={detailsConversation}
              rename={tabDetails.rename}
              onClose={() => setTabDetails(null)}
              onRename={(title) => chat.rename(tabDetails.id, title)}
            />
          )}
          <div className="nessa-composer">
            <ConversationNotification
              conversation={chat.active}
              connection={session}
              gatewayAvailable={chat.gatewayAvailable}
            />
            <ConversationQueue
              key={chat.active.id}
              conversation={chat.active}
              gatewayAvailable={chat.gatewayAvailable}
            />
            {generating && chat.active.remote?.capabilities.steer && (
              <ComposerDeliveryMode
                className="mb-2"
                value={chat.deliveryMode}
                onValueChange={chat.setDeliveryMode}
                disabled={!!chat.active.controlPending}
              />
            )}
            {sessionError && (
              <p role="alert" className="px-3 nessa-text-4 text-destructive">
                {sessionError}
              </p>
            )}
            {attachments.error && (
              <p role="alert" className="px-3 nessa-text-4 text-destructive">
                {attachments.error}
              </p>
            )}
            {attachments.reading && (
              <p role="status" className="px-3 nessa-text-4 text-muted-foreground">
                Reading files…
              </p>
            )}
            <PillComposer
              key={chat.active.id}
              expandable={viewedPaste === null && attachments.viewed === null}

              generating={false}
              onSubmit={submit}
            >
              <ChatComposerAttachments>
                {attachments.pendingFiles.map((file, index) => (
                  <span
                    key={index}
                    className="relative m-1 inline-flex"
                    aria-label={`Reading ${file.name}`}
                    aria-busy="true"
                  >
                    <ChatAttachmentTile
                      label={file.name}
                      imageSrc={file.previewUrl}
                      icon={<AttachmentIcon name={file.name} mimeType={file.mimeType} />}
                    />
                  </span>
                ))}
                {attachments.files.map((file) => (
                  <span key={file.id} className="relative m-1 inline-flex">
                    <ChatAttachmentTile
                      label={file.name}
                      imageSrc={
                        file.mimeType.startsWith("image/") ? file.previewUrl : undefined
                      }
                      icon={<AttachmentIcon name={file.name} mimeType={file.mimeType} />}
                      onOpen={() => attachments.open(file)}
                    />
                    <button
                      type="button"
                      aria-label={`Remove ${file.name}`}
                      title={`Remove ${file.name}`}
                      onClick={() => attachments.remove(file.id)}
                      className="absolute -right-1.5 -top-1.5 inline-flex size-5 items-center justify-center rounded-full bg-background text-foreground shadow-sm outline-none focus-visible:[outline-style:solid] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring [&_svg]:size-3"
                    >
                      <X aria-hidden="true" />
                    </button>
                  </span>
                ))}
              </ChatComposerAttachments>
              <PillComposerRow>
                <AddAttachmentMenu
                  disabled={attachments.reading}
                  onChoose={attachments.chooseFiles}
                  onSignOut={onSignOut}
                />
                <ChatComposerMarkdownEditor
                  key={`${chat.active.id}:${chat.active.turns.filter((turn) => turn.from === "user").length}:${chat.active.draftReset ?? 0}`}
                  ref={setComposerRef}
                  defaultContent={toEditor(chat.active.draft)}
                  onContentChange={changeContent}
                  onChipPress={pressChip}
                  pasteAttachmentMinLength={500}
                  onPasteAttachment={pasteAttachment}
                  onPasteFiles={(files) => void attachments.addFiles(files)}
                  placeholder="Ask me anything"
                  aria-label="Message"
                  maxHeight={240}
                />
                {/* Enter sends; Shift+Enter starts a new Markdown block.
                  Voice stays visible while typing and remains inert until wired. */}
                {generating ? (
                  <ChatComposerAction
                    className="nessa-composer-control"
                    aria-label="Stop active and queued work"
                    title="Stop active and queued work"
                    disabled={!chat.gatewayAvailable}
                    onClick={chat.stopGenerating}
                  >
                    <Square aria-hidden="true" className="fill-current" />
                  </ChatComposerAction>
                ) : (
                  <ChatComposerAction
                    className="nessa-composer-control"
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
      </FileDropZone>
    </div>
  )
}
