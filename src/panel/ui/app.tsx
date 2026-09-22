import { ComposerDeliveryMode } from "@nessa-ui/react/composer-queue"
import type { AttachmentResources } from "../adapters/attachment-resources"
import * as React from "react"
import { CircleArrowUp, Download, KeyRound, Link2Off, Square } from "lucide-react"
import { AgentNotification } from "@nessa-ui/react/agent-notification"
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
import { useUpdate } from "../adapters/use-update"
import { usePanelLinkNotice } from "../adapters/use-link-notice"
import { UPDATE_TAB_ID } from "../application/update-surface"
import { tabAfter, tabAt } from "../application/tab-navigation"
import { UpdateTab } from "./update-tab"
import { useComposer } from "./use-composer"
import { useFileAttachments } from "./use-file-attachments"
import { useAttachmentUploads } from "./use-attachment-uploads"
import { useFolderDrop } from "./use-folder-drop"
import { useContentDrop } from "./use-content-drop"
import { useHostDrop } from "./use-host-drop"
import { ChatAttachmentTile } from "@nessa-ui/react/chat-bubbles"

import { AddAttachmentMenu } from "./add-attachment-menu"
import { AttachmentDropZone } from "./attachment-drop-zone"
import { AttachmentNotices, AttachmentReadingStatus } from "./attachment-notices"
import { AttachmentTile } from "./attachment-tile"
import { AttachmentIcon } from "./attachment-icon"
import { ComposerNotices } from "./composer-notices"
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
  canChoosePaths,
  digest,
  onSignOut,
  sessionError,
}: {
  attachmentResources: AttachmentResources
  /**
   * Whether this surface has a picker that can say where a file is. False in a
   * browser, where every route hands over bytes and none says their location.
   *
   * Taken rather than asked for. The chrome used to put the question to the
   * host itself while composition put it again for the send refusal, and one
   * fact with two readers is a seam that generates the disagreement it was
   * supposed to prevent — which it did: a browser was told to press `+` by one
   * of them and that `+` would not work by the other. `check-architecture`
   * keeps the question in composition.
   */
  canChoosePaths: boolean
  /** How an upload's bytes are identified. Composition owns the Web Crypto one. */
  digest: (bytes: Blob) => Promise<string>
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
  const uploads = useAttachmentUploads(chat, attachmentResources, digest)
  const folderDrop = useFolderDrop(
    chat.active.id,
    attachments.addFiles,
    attachments.refuse,
  )
  const session = useSession()
  // Panel-level, not conversation-level: an available update is a fact about
  // the application, it outlives every tab somebody opens or closes, and both
  // of its surfaces — the notice over the composer and the tab beside the
  // conversation tabs — are the panel's own chrome.
  const update = useUpdate()
  // A link leaves the app rather than loading in here, so when one goes
  // nowhere the click is otherwise indistinguishable from a dead panel.
  const link = usePanelLinkNotice()
  const {
    expanded,
    changeExpanded,
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
  } = useComposer(
    chat,
    (id) => {
      if (!attachments.isPending(id) && !folderDrop.isPending(id)) return false
      attachments.refuse({ reason: "sending-while-reading" })
      return true
    },
    attachments.draftSent,
  )
  const contentDrop = useContentDrop({
    addFolderEntries: folderDrop.addFolderEntries,
    addImageUrl: attachments.addImageUrl,
    focusComposer,
    pasteAttachment,
  })
  // Two sources, one at a time. In the app the host owns the drag — it is the
  // only thing that can learn a dropped file's path — and the page receives no
  // drop events at all, so `contentDrop`'s handlers never fire. In a browser
  // there is no host, the page keeps its own drops, and nothing below fires.
  // Neither is gated on the other: each is silent where the other is live.
  const hostDrop = useHostDrop({
    addChosenFiles: (chosen, conversationId) =>
      void attachments.addChosenFiles(chosen, conversationId),
    addImageUrl: (url) => void attachments.addImageUrl(url),
    conversationOf: attachments.conversationOf,
    focusComposer,
    pasteAttachment,
    refuse: attachments.refuse,
  })
  /**
   * Show a conversation, whatever made it the one to show.
   *
   * Selecting a conversation is also leaving the update tab and closing the
   * pasted-text viewer: the strip is one strip, and a selection that left the
   * update on screen would be selecting nothing. That is three calls, and there
   * are eight ways to select a conversation — a shortcut, a click, the tab
   * menu, a new tab — so they go through one door rather than being remembered
   * at each; `show` is only which conversation.
   */
  const showConversation = React.useEffectEvent((show: () => void) => {
    closePaste()
    update.setViewing(false)
    show()
  })
  const openTab = React.useEffectEvent(() =>
    showConversation(() => {
      chat.openConversation()
      focusComposer()
    }),
  )
  const closeActiveTab = React.useEffectEvent(() => {
    closePaste()
    // The update tab closes like any other. What that means depends on what it
    // was doing — a running download is hidden rather than stopped, a finished
    // one is turned down — and `afterUpdate` owns that, not this.
    if (update.viewing) {
      update.close()
      return
    }
    chat.closeConversation(chat.active.id)
  })
  /**
   * The strip as a person sees it, which is what the keyboard moves through.
   *
   * The update tab is in it, so navigating `chat.conversations` alone stepped
   * over the update and started from a conversation that was not what was
   * selected — next from the last conversation went back to the first rather
   * than to the update, and a numbered shortcut aimed at the update's position
   * found nothing there.
   */
  const stripIds = React.useMemo(
    () => [
      ...chat.conversations.map((item) => item.id),
      ...(update.tab ? [UPDATE_TAB_ID] : []),
    ],
    [chat.conversations, update.tab],
  )
  const selectedId = update.viewing ? UPDATE_TAB_ID : chat.active.id

  /** Show whichever tab the strip named, update or conversation. */
  const showTab = React.useEffectEvent((id: string | undefined) => {
    if (id === undefined) return
    if (id === UPDATE_TAB_ID) {
      closePaste()
      update.setViewing(true)
      return
    }
    showConversation(() => chat.setActive(id))
  })

  const activateTab = React.useEffectEvent(
    (target: { index?: number; conversationId?: string }) => {
      // A command naming a conversation means that conversation, wherever it
      // sits; only the positional forms count tabs.
      const named = target.conversationId
      if (named && chat.conversations.some((item) => item.id === named)) {
        showConversation(() => chat.setActive(named))
        return
      }
      if (typeof target.index !== "number") return
      showTab(tabAt(stripIds, target.index))
    },
  )
  const moveActiveTab = React.useEffectEvent((direction: -1 | 1) =>
    showTab(tabAfter(stripIds, selectedId, direction)),
  )
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
  // After the conversations, because it arrived after them and because a strip
  // that reorders itself under somebody's pointer is worse than a long one.
  if (update.tab) {
    tabs.push({
      id: UPDATE_TAB_ID,
      title: "Update",
      closeable: update.tab.closeable,
      icon: <span aria-hidden="true" className="nessa-update-tab-dot" />,
    })
  }

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
      <AttachmentDropZone onFiles={attachments.addFiles} onRefused={attachments.refuse}>
        <div
          ref={edge.panelRef}
          data-nessa-root
          data-content-dragging={contentDrop.dragging || hostDrop.dragging || undefined}
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
              wrapTab={(tab, node) =>
                // The update has no title to rename and no agent to describe,
                // so the conversation menu does not belong on it.
                tab.id === UPDATE_TAB_ID ? (
                  node
                ) : (
                  <ConversationTabMenu
                    key={tab.id}
                    onDetails={() =>
                      showConversation(() => {
                        chat.setActive(tab.id)
                        setTabDetails({ id: tab.id, rename: false })
                      })
                    }
                    onRename={() => setTabDetails({ id: tab.id, rename: true })}
                  >
                    {node}
                  </ConversationTabMenu>
                )
              }
              label="Conversations"
              tabs={tabs}
              value={update.viewing ? UPDATE_TAB_ID : chat.active.id}
              onValueChange={(id) => {
                if (id === UPDATE_TAB_ID) {
                  closePaste()
                  update.setViewing(true)
                  return
                }
                showConversation(() => chat.setActive(id))
              }}
              onClose={(id) => {
                closePaste()
                if (id === UPDATE_TAB_ID) {
                  update.close()
                  return
                }
                chat.closeConversation(id)
              }}
              onNew={() => openTab()}
              newTabLabel="New conversation"
            />
          </div>

          {/* Keyed so a layout compositor drops the previous conversation's
            tiles instead of leaving them over the wallpaper. */}
          <div className="nessa-transcript-region relative flex min-h-0 flex-1 flex-col">
            {update.viewing && update.tab ? (
              <UpdateTab tab={update.tab} onRetry={update.install} />
            ) : (
              <>
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
                {!attachments.viewed &&
                  viewedPaste?.conversationId === chat.active.id && (
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
              </>
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
          {/* Hidden rather than unmounted while the update tab is up: the
              draft, the attachments, and the caret are all in here, and a
              detour through an update must not cost somebody their message. */}
          <div className="nessa-composer" hidden={update.viewing || undefined}>
            {/* Everything said above the pill goes through one box, which owns
                the order it is said in and the room it may take. A notice added
                straight to this element instead would be unbounded again, and
                would put itself wherever it was pasted; the architecture check
                refuses that. The queue badge and the delivery row stay outside:
                they are controls rather than statements, they are one short row
                each, and a control the composer is about to obey must not be
                somewhere you have to scroll to find. */}
            <ComposerNotices
              link={
                /* The same surface the connection notice uses, carrying a
                   notice that is not about the connection. `state` is the
                   component's connection vocabulary, and "disconnected" is the
                   only one of the four that renders the primary action at all —
                   so a notice with something to do declares itself disconnected
                   whatever it is about, exactly as the conversation notices
                   below already do. The heading, glyphs, and labels here are
                   all ours; nothing of the connection wording survives. */
                link.notice ? (
                  <AgentNotification
                    className="mb-2"
                    state="disconnected"
                    icon={Link2Off}
                    title={link.notice.title}
                    description={link.notice.description}
                    dismissLabel="Dismiss"
                    onDismiss={link.dismiss}
                  />
                ) : null
              }
              update={
                update.notice ? (
                  <AgentNotification
                    className="mb-2"
                    state="disconnected"
                    icon={CircleArrowUp}
                    title="Update available"
                    description={update.notice.version}
                    retryIcon={Download}
                    retryLabel={update.notice.installLabel}
                    onRetry={update.install}
                    dismissLabel={update.notice.dismissLabel}
                    onDismiss={update.dismiss}
                  />
                ) : null
              }
              attachments={
                /* What the draft's files need said about them, and why the last
                   thing offered was turned away — both, when both are true. The
                   tile already marks a failed upload and carries the full
                   reason, so this does not repeat it; it offers the retry once
                   for every upload worth retrying. */
                <AttachmentNotices
                  canChoosePaths={canChoosePaths}
                  refusal={attachments.refusal}
                  files={attachments.files}
                  imageInput={chat.active.remote?.capabilities.imageInput}
                  onRetryUploads={(files) => files.forEach(uploads.retry)}
                  onChooseFiles={attachments.chooseFiles}
                  onDismissRefusal={attachments.clearRefusal}
                />
              }
              session={
                /* Not the connection's own notice below, which the session's
                   phase owns: this is what the surface around the session could
                   not do — restoring a sign-in, ending one — and it is said on
                   the same surface rather than as red text under the composer. */
                sessionError ? (
                  <AgentNotification
                    className="mb-2"
                    state="disconnected"
                    // Not the state's own aerial: nothing here is a connection.
                    // These are sign-ins that could not be restored or ended.
                    icon={KeyRound}
                    title="Session needs attention"
                    description={sessionError}
                  />
                ) : null
              }
              conversation={
                <ConversationNotification
                  conversation={chat.active}
                  connection={session}
                  gatewayAvailable={chat.gatewayAvailable}
                />
              }
            />
            {/* Keyed per conversation so the queue is rebuilt rather than
                carried across a tab change, and named apart from the composer
                below, which is keyed by the same conversation. Two children of
                one parent under one key are one child to React. That alone is
                survivable; here it was not, because the notices above render
                nothing most of the time, and a falsy sibling earlier in the
                array makes React leak one subtree per render instead of
                reusing it. The composer filled with queue chips, each frozen
                at the count it was born with, none removed when the queue
                drained. See src/panel/ui/composer-keys.test.tsx. */}
            <ConversationQueue
              key={`queue:${chat.active.id}`}
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
            <PillComposer
              key={`composer:${chat.active.id}`}
              expandable={viewedPaste === null && attachments.viewed === null}
              // Controlled, so the pane survives a submit this panel turned
              // away — an attachment still reading, an empty draft — and closes
              // only once a message has actually gone. See `useComposer`.
              expanded={expanded}
              onExpandedChange={changeExpanded}
              generating={false}
              onSubmit={submit}
            >
              {/* Transient, and not a problem, so it is neither a notice nor a
                  paragraph beside one: the visible half is the busy tile below,
                  and this is the same thing for somebody who cannot see it. */}
              <AttachmentReadingStatus reading={attachments.pendingFiles.length > 0} />
              <ChatComposerAttachments>
                {attachments.pendingFiles.map((file, index) => (
                  <span
                    key={index}
                    className="relative m-1 inline-flex"
                    aria-label={`Getting ${file.name} ready`}
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
                  <AttachmentTile
                    key={file.id}
                    file={file}
                    onOpen={() => attachments.open(file)}
                    onRemove={() => attachments.remove(file.id)}
                    onRetry={() => uploads.retry(file.id)}
                  />
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
      </AttachmentDropZone>
    </div>
  )
}
