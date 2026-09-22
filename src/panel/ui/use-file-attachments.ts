import * as React from "react"
import {
  declaredMediaType,
  isImageFile,
  linkablePath,
  linkedFile,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  type FileAttachment,
  type useConversation,
} from "../../conversation"
import {
  chooseAttachmentFiles,
  onAttachmentBatch,
  onAttachmentReadying,
  readAttachmentBytes,
  type ChosenFile,
} from "../../host"
import { readDroppedImage } from "../adapters/dropped-image"
import {
  MAX_SESSION_ATTACHMENT_BYTES,
  type AttachmentResources,
} from "../adapters/attachment-resources"
import { pickerRefusal, type AttachmentRefusal } from "../application/attachment-notice"

const MIB = 1024 * 1024

/**
 * How many gestures the panel remembers the conversation of.
 *
 * One entry is two short strings, and nothing removes one when its attach
 * finishes — an attach can end in files, in a refusal, or in a conversation
 * that has since closed, and only the first of those comes back here. Rather
 * than three cleanup paths that a fourth outcome would quietly escape, the map
 * is a window over the most recent gestures.
 *
 * Far more than can ever be in flight: the longest an attach waits is
 * forty-five seconds, and nobody drops sixty-four times in forty-five seconds.
 * An entry evicted while still in flight falls back to the open tab, which is
 * what the panel did for every gesture before any of this existed.
 */
const MOST_REMEMBERED_BATCHES = 64

/** Coordinates local file selection and preview against the originating conversation. */
/**
 * Fetch the preview's own chunk before anybody asks for it.
 *
 * Opening a file is one click with nothing before it, and the preview arrives
 * with the design system's whole set of renderers behind it — Markdown, JSON,
 * PDF, video — because the one that draws a picture is registered beside them.
 * Waiting for all that after the click is a sheet that says "Loading preview…"
 * for as long as the fetch takes.
 *
 * Attaching a file is the moment somebody might open one, and it is a moment
 * with nothing else going on, so the chunk is fetched then and the click has
 * nothing left to wait for. Repeat calls cost nothing: the same specifier
 * resolves to the module already in hand, which is also what lets this and the
 * panel's `React.lazy` of it agree.
 */
const warmPreview = () => void import("./attachment-preview")

export function useFileAttachments(
  chat: ReturnType<typeof useConversation>,
  resources: AttachmentResources,
) {
  const conversationsRef = React.useRef(chat.conversations)
  React.useLayoutEffect(() => {
    conversationsRef.current = chat.conversations
  }, [chat.conversations])
  const inputRef = React.useRef<HTMLInputElement>(null)
  const [pending, setPending] = React.useState<{
    conversationId: string
    files: { name: string; mimeType: string; previewUrl?: string }[]
  } | null>(null)
  /**
   * Files the host is making readable: its identity for each one, and what to
   * put on the tile.
   *
   * **Keyed on the host's identity, never on the name.** Two folders can each
   * hold a `report.pdf`, and a map keyed on the name had one entry for both:
   * the first to settle took the other's tile away, and the draft became
   * sendable while the second was still being fetched.
   *
   * Their own state rather than `pending` above, which one call owns from
   * beginning to end. These arrive and leave on the host's schedule, one event
   * each way.
   */
  const [readying, setReadying] = React.useState<
    Map<string, { name: string; conversationId: string }>
  >(new Map())
  /**
   * Which conversation each batch of work belongs to.
   *
   * Recorded when the *gesture* happens — the drop, or the moment `+` is
   * pressed — and not when the host finally speaks, which can be three
   * quarters of a minute later. Reading the open tab then put the tile over
   * whichever conversation somebody had since switched to: it blocked *that*
   * draft's send for forty-five seconds for a file it was never going to get,
   * while the draft the file was actually joining showed nothing at all.
   */
  const batches = React.useRef(new Map<string, string>())
  /**
   * The draft a `+` was pressed in, while its picker is open.
   *
   * Null the rest of the time, and a drop never reads it — the host says which
   * gesture a name belongs to, so a drop landing during a pick still binds to
   * the tab it landed on.
   */
  const picking = React.useRef<string | null>(null)
  const download = React.useRef<AbortController | null>(null)
  const busyRef = React.useRef(false)
  const pendingConversation = React.useRef<string | null>(null)
  const mounted = React.useRef(true)
  const [reading, setReading] = React.useState(false)
  /**
   * Why the last thing offered was not taken, and which conversation it was
   * said to.
   *
   * Typed rather than a sentence: the words, and whether there is anything to
   * be done, belong to `attachment-notice`.
   *
   * A refusal answers one attempt and lives until something answers it back —
   * and what those things are is written down here as calls to
   * {@link answerRefusal}, not inferred from the draft afterwards. Inferring
   * was tried and is wrong in both directions: a comparison taken while the
   * handler runs is older than the attach happening in the same handler, so a
   * drop that attached one file and refused another said nothing at all; and a
   * draft restored after a refused send brings its files back under the same
   * identities, so a refusal that had been answered came back with them.
   */
  const [refused, setRefused] = React.useState<{
    conversationId: string
    refusal: AttachmentRefusal
  } | null>(null)
  const [viewed, setViewed] = React.useState<{
    conversationId: string
    file: FileAttachment
  } | null>(null)
  React.useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      download.current?.abort()
    }
  }, [])
  const activeId = React.useRef(chat.active.id)
  React.useLayoutEffect(() => {
    activeId.current = chat.active.id
  })
  /**
   * The conversation a batch belongs to. The host's identity for a waiting
   * file begins with its batch, so the tile finds its draft through the
   * gesture that started it rather than through whatever is on screen now.
   *
   * A batch nobody recorded falls back to the open tab. That is a real
   * fallback rather than the ordinary path — both gestures announce
   * themselves — and it covers the panel having reloaded mid-attach, where
   * the open tab is the only thing left to go on.
   */
  const conversationOf = (id: string) =>
    batches.current.get(id.split(":")[0] ?? "") ?? activeId.current
  React.useEffect(() => {
    let live = true
    // The gesture's own moment, for both gestures. Everything after it can be
    // three quarters of a minute later, and the open tab is no longer evidence
    // of anything by then.
    const batch = onAttachmentBatch((named, gesture) => {
      if (!live) return
      // A `+` selection was begun here, and `chooseFiles` has been holding the
      // draft it was begun on ever since; the host's word arrives when the
      // picker closes. Answering with the press is one source of truth for the
      // files *and* the tile — reading the tab again was a second, and the two
      // agreed only while nothing could change between the press and the close.
      // What guaranteed that was the picker being modal, which is rfd's
      // presentation choice and not a promise to us.
      const began = gesture === "picked" ? picking.current : null
      // Bounded: the oldest are dropped once there are more than a window's
      // worth of gestures in flight, because nothing removes an entry when an
      // attach finishes and a long session would otherwise keep one string
      // pair per drop and per `+` for as long as the window lives. Insertion
      // order is age here — the host's counter never reuses a name.
      const held = batches.current
      held.set(named, began ?? activeId.current)
      for (const oldest of held.keys()) {
        if (held.size <= MOST_REMEMBERED_BATCHES) break
        held.delete(oldest)
      }
    })
    const subscription = onAttachmentReadying(({ id, name, readying: waiting }) => {
      if (!live) return
      setReadying((current) => {
        const next = new Map(current)
        // Settling always removes, whatever the outcome was: the sentence for
        // a file that is not coming is the refusal's, and a tile left spinning
        // beside it would be the panel disagreeing with itself.
        if (waiting) next.set(id, { name, conversationId: conversationOf(id) })
        else next.delete(id)
        return next
      })
    })
    return () => {
      live = false
      void batch.then((stop) => stop())
      void subscription.then((stop) => stop())
    }
  }, [])
  React.useEffect(() => {
    if (
      viewed &&
      !chat.conversations.some(
        (conversation) =>
          conversation.id === viewed.conversationId &&
          conversation.draft.some(
            (part) => part.type === "file" && part.id === viewed.file.id,
          ),
      )
    )
      setViewed(null)
  }, [chat.conversations, viewed])
  // Somebody with a file attached is somebody who may open it.
  const anyFiles = chat.conversations.some((conversation) =>
    conversation.draft.some((part) => part.type === "file"),
  )
  React.useEffect(() => {
    if (anyFiles) warmPreview()
  }, [anyFiles])
  const files = chat.active.draft.filter(
    (part): part is FileAttachment => part.type === "file",
  )
  /**
   * Say why something was not taken. Defaults to the active conversation; a
   * folder walk that finishes after somebody has moved on names the one it was
   * dropped into, so its refusal is not shown above another draft quoting
   * another draft's bounds.
   *
   * Replaces whatever was there. There is one of these at a time, and it is
   * always the most recent thing the panel turned away.
   */
  const refuse = (refusal: AttachmentRefusal, conversationId = chat.active.id) =>
    setRefused({ conversationId, refusal })
  /**
   * Put down a refusal this conversation has now answered, leaving another
   * tab's alone. The answers are: files actually attached, a file taken off the
   * draft, and the draft going to the gateway. Each of them is the person
   * having moved on from the attempt the refusal was about.
   *
   * Written as an update rather than a read, so it is correct from inside an
   * event handler that is also attaching — which is where the ordering matters:
   * the drop zone hands over what passed before what it refused, and the
   * refusal must be what survives the pair.
   */
  const answerRefusal = (conversationId: string) =>
    setRefused((current) => (current?.conversationId === conversationId ? null : current))
  /** Said only to the conversation it was said about. */
  const refusal = refused?.conversationId === chat.active.id ? refused.refusal : null
  /**
   * Put files on a draft, whatever route they came in by.
   *
   * `held` are files this window has the bytes of and `linked` are files it
   * knows only the location of, and the split between them is the file's type
   * and never the gesture: an image is uploaded and everything else is pointed
   * at by path. Both lists are attached in one call, so a selection lands whole
   * or not at all.
   *
   * Which bound applies to which is the whole reason they are told apart. The
   * per-file, per-draft and per-window byte budgets all exist because this
   * window would otherwise be holding those bytes, so they are counted over
   * `held` alone — a 700 MB video that travels as a path is an ordinary
   * attachment. How many files one draft shows at once is about the draft, and
   * counts both.
   */
  function attach(
    held: readonly File[],
    linked: readonly ChosenFile[],
    targetId: string,
  ) {
    if (!held.length && !linked.length) return
    if (busyRef.current) {
      refuse({ reason: "reading-files" }, targetId)
      return
    }
    const target = conversationsRef.current.find(
      (conversation) => conversation.id === targetId,
    )
    // The draft these were for is gone — closed during a fetch that can take
    // three quarters of a minute, or during the read that follows it. There is
    // nothing to attach them to, and saying so on the conversation that no
    // longer exists would be saying it to nobody, so it is said where the
    // person is looking.
    //
    // It is said at all because the alternative is the silence this module
    // refuses everywhere else: a person who chose five files and got none has
    // been told nothing about any of them. This used to `return` here.
    //
    // This is the authority, and `addChosenFiles` only short-circuits ahead of
    // it: every route into a draft passes here, including the ones that never
    // read a byte.
    if (!target) {
      refuse({ reason: "conversation-closed" }, activeId.current)
      return
    }
    // Three states, and only one of them refuses. `imageInput` is the
    // gateway's answer about *this* conversation's model, and `undefined`
    // means it has not answered yet — which is not "no". Guessing "no" would
    // turn away images an agent takes; guessing "yes" is what this is fixing,
    // because an image attached to a model that takes none was uploaded in
    // full and then shown in red as though the upload had gone wrong. Nothing
    // had gone wrong: it was never going to work, and it was knowable before
    // a single byte moved. The send path has taken exactly this care about
    // `undefined` since images shipped; this is the other half of it.
    //
    // Read from the target rather than the active conversation: a folder walk
    // can finish after somebody has moved on, and the answer that matters is
    // the one for the draft the files are landing on.
    if (
      target.remote?.capabilities.imageInput === false &&
      held.some((file) => isImageFile(declaredMediaType(file.name, file.type)))
    ) {
      refuse({ reason: "images-not-supported" }, targetId)
      return
    }
    const targetFiles = target.draft.filter((part) => part.type === "file")
    // Three bounds, told apart, because the advice for each is different and
    // one of them has no advice at all: a file over the per-file bound is
    // refused by every route that carries bytes.
    const tooLarge = held.filter((file) => file.size > MAX_ATTACHMENT_BYTES)
    if (tooLarge.length > 0) {
      refuse(
        {
          reason: "file-too-large",
          files: tooLarge.map((file) => ({ name: file.name, type: file.type })),
        },
        targetId,
      )
      return
    }
    if (targetFiles.length + held.length + linked.length > MAX_DRAFT_ATTACHMENTS) {
      refuse({ reason: "too-many-files" }, targetId)
      return
    }
    const carrying = targetFiles.filter((file) => !linkedFile(file))
    if (
      [...carrying, ...held].reduce((total, file) => total + file.size, 0) >
      MAX_DRAFT_ATTACHMENT_BYTES
    ) {
      refuse({ reason: "draft-too-large" }, targetId)
      return
    }
    if (!resources.canAdd(held.reduce((total, file) => total + file.size, 0))) {
      refuse(
        {
          reason: "window-budget",
          maxMiB: MAX_SESSION_ATTACHMENT_BYTES / MIB,
          draftsHoldFiles: conversationsRef.current.some((conversation) =>
            conversation.draft.some((part) => part.type === "file"),
          ),
        },
        targetId,
      )
      return
    }
    answerRefusal(targetId)
    try {
      // Object URLs are ready synchronously; no FileReader or duplicate thumbnail URLs.
      chat.attachFiles([...resources.add(held), ...resources.addChosen(linked)], targetId)
    } catch {
      refuse({ reason: "unreadable-files" }, targetId)
    }
  }

  function addFiles(selected: readonly File[], targetId = chat.active.id) {
    attach(selected, [], targetId)
  }

  /**
   * Ask for files to attach, through the host's own picker where there is one.
   *
   * The picker is the only thing that can say where a file is, and that is what
   * makes a file that is not an image sendable at all. Its answer is a list of
   * paths, so the images among them still have to be read before they can be
   * uploaded — the type decides the route, and a picked image is as much an
   * image as a dropped one.
   *
   * Without a host — a browser, `pnpm dev` — there is no picker, and the page's
   * own file input is used instead. That reads bytes and never says where they
   * came from, so images work there and nothing else can be sent.
   */
  async function chooseFiles() {
    const targetId = chat.active.id
    // Held for the host's word, which comes when the picker closes and carries
    // no conversation of its own. One capture answers both the files below and
    // the tile the host draws in between.
    picking.current = targetId
    let chosen
    try {
      chosen = await chooseAttachmentFiles()
    } catch (error) {
      refuse(pickerRefusal(error), targetId)
      return
    } finally {
      picking.current = null
    }
    if (chosen === null) {
      inputRef.current?.click()
      return
    }
    await addChosenFiles(chosen, targetId)
  }

  /** Whether a conversation is still open to attach to. */
  const stillOpen = (conversationId: string) =>
    conversationsRef.current.some((conversation) => conversation.id === conversationId)

  /**
   * Attach what the picker chose: read the images, point at everything else.
   *
   * Reading is why this waits. A selection is attached whole or not at all, so
   * one image that will not read turns the whole selection away with its
   * reason rather than leaving a draft short of what was picked.
   */
  async function addChosenFiles(
    chosen: readonly ChosenFile[],
    targetId = chat.active.id,
  ) {
    if (!chosen.length) return
    if (busyRef.current) {
      refuse({ reason: "reading-files" }, targetId)
      return
    }
    // Checked before the read as well as inside `attach`, which is what makes
    // the common case cheap: a selection can be forty-five seconds behind its
    // gesture, and a draft closed in that time has nothing to read images for.
    // It is a short-circuit, not the authority — a conversation closed *during*
    // the read still reaches `attach`, and the bytes for that one are spent and
    // thrown away. Both end in the same sentence.
    if (!stillOpen(targetId)) {
      refuse({ reason: "conversation-closed" }, activeId.current)
      return
    }
    // The platform's own answer first, the extension table only as the
    // fallback — the same call, with the same inputs, that a dropped or pasted
    // file goes through. Classifying a picked file from the table alone was how
    // `.ico`, `.jpe`, `.svgz`, `.jp2`, `.xbm`, `.tga` and `.dib` came to upload
    // when dropped and travel as a path when picked: one file, two routes,
    // which is the one thing this rule forbids.
    const image = (file: ChosenFile) =>
      isImageFile(declaredMediaType(file.name, file.mimeType))
    const images = chosen.filter(image)
    const linked = chosen.filter((file) => !image(file))
    // A path the gateway would refuse is refused here instead, while the file
    // can still be swapped, rather than at send.
    const unusable = linked.find((file) => !linkablePath(file.path))
    if (unusable) {
      refuse({ reason: "file-not-linkable", name: unusable.name }, targetId)
      return
    }
    if (!images.length) {
      attach([], linked, targetId)
      return
    }
    busyRef.current = true
    setReading(true)
    let held: File[]
    try {
      held = await Promise.all(
        images.map(async (image) => {
          // The ticket, never the path: the host holds what it minted one for,
          // so this page cannot ask it to open a file nobody chose.
          const bytes = await readAttachmentBytes(image.ticket)
          if (bytes === null) throw new Error("no native host to read a file with")
          return new File([bytes], image.name, {
            type: declaredMediaType(image.name, image.mimeType),
          })
        }),
      )
    } catch (error) {
      busyRef.current = false
      if (mounted.current) {
        setReading(false)
        refuse(pickerRefusal(error), targetId)
      }
      return
    }
    busyRef.current = false
    if (!mounted.current) return
    setReading(false)
    attach(held, linked, targetId)
  }

  async function addImageUrl(url: string) {
    if (busyRef.current) return
    const targetId = chat.active.id
    busyRef.current = true
    pendingConversation.current = targetId
    setReading(true)
    answerRefusal(targetId)
    setPending({
      conversationId: targetId,
      files: [{ name: "Image", mimeType: "image/" }],
    })
    const controller = new AbortController()
    download.current = controller
    try {
      const file = await readDroppedImage(url, controller.signal, MAX_ATTACHMENT_BYTES)
      if (!mounted.current) return
      busyRef.current = false
      setReading(false)
      setPending(null)
      addFiles([file], targetId)
    } catch {
      if (mounted.current) refuse({ reason: "unreadable-image-url" }, targetId)
    } finally {
      download.current = null
      pendingConversation.current = null
      busyRef.current = false
      if (mounted.current) {
        setReading(false)
        setPending(null)
      }
    }
  }
  return {
    inputRef,
    /**
     * Whether this conversation is waiting on something that has to finish
     * before a draft can go — an image being fetched from a URL, or a file the
     * host is still making readable. The composer refuses a send while either
     * is true, with the words it already has for it.
     */
    isPending: (conversationId: string) =>
      pendingConversation.current === conversationId ||
      [...readying.values()].some((file) => file.conversationId === conversationId),
    /**
     * Tiles for files that are on their way but not attached. Both kinds
     * together, because they are the same thing to look at and the same thing
     * to wait for; neither claims the file is on the draft.
     */
    pendingFiles: [
      ...(pending?.conversationId === chat.active.id ? pending.files : []),
      ...[...readying.values()]
        .filter((file) => file.conversationId === chat.active.id)
        .map((file) => ({ name: file.name, mimeType: "", previewUrl: undefined })),
    ],
    files,
    reading,
    refusal,
    refuse,
    /**
     * The draft has gone to the gateway, so whatever was refused before it is
     * over. Called for a submission that was actually taken — a send the panel
     * turned away has not moved anybody on, and is usually itself the refusal
     * being shown.
     */
    draftSent: answerRefusal,
    clearRefusal: () => setRefused(null),
    addFiles,
    addImageUrl,
    conversationOf,
    chooseFiles: () => void chooseFiles(),
    addChosenFiles,
    viewed: viewed?.conversationId === chat.active.id ? viewed.file : null,
    open: (file: FileAttachment) => setViewed({ conversationId: chat.active.id, file }),
    close: () => setViewed(null),
    remove: (id: string) => {
      chat.removeFile(id)
      // Taking a tile off is an answer to "attach fewer" and to "this draft is
      // too heavy" alike, whichever of them was being said.
      answerRefusal(chat.active.id)
      if (viewed?.file.id === id) setViewed(null)
    },
  }
}
