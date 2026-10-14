import * as React from "react"
import { useStore } from "react-redux"
import {
  type AttachmentResource,
  declaredMediaType,
  isImageFile,
  linkablePath,
  linkedFile,
  linksFilesHere,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  type FileAttachment,
  type useConversation,
} from "../../conversation"
import type { AppStore } from "../../store"
import {
  chooseAttachmentFiles,
  onAttachmentBatch,
  onAttachmentReadying,
  readAttachmentBytes,
  type ChosenFile,
  type PickerRequestId,
} from "../../host"
import { readDroppedImage } from "../adapters/dropped-image"
import {
  MAX_SESSION_ATTACHMENT_BYTES,
  type AttachmentResources,
} from "../adapters/attachment-resources"
import { pickerRefusal, type AttachmentRefusal } from "../application/attachment-notice"

const MIB = 1024 * 1024

/** How many host-minted drops may wait for their terminal result at once. */
const MAX_ACTIVE_DROP_BATCHES = 64

export type AttachmentDestination = Readonly<{
  store: AppStore
  conversationId: string
  resources: AttachmentResources
  attachFiles: (files: AttachmentResource[], conversationId: string) => void
}>

type AttachmentOperation = Readonly<{
  destination: AttachmentDestination
  files: { name: string; mimeType: string; previewUrl?: string }[]
  controller?: AbortController
  pickerRequestId?: PickerRequestId
}>

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
  const store = useStore() as AppStore
  const currentScope = React.useRef({ store, resources, attachFiles: chat.attachFiles })
  React.useLayoutEffect(() => {
    currentScope.current = { store, resources, attachFiles: chat.attachFiles }
  }, [chat.attachFiles, resources, store])
  const activeId = React.useRef(chat.active.id)
  React.useLayoutEffect(() => {
    activeId.current = chat.active.id
  }, [chat.active.id])
  const currentDestination = React.useCallback(
    (conversationId = activeId.current): AttachmentDestination => {
      const scope = currentScope.current
      return Object.freeze({ ...scope, conversationId })
    },
    [],
  )
  const destinationIsCurrent = React.useCallback(
    (destination: AttachmentDestination) =>
      destination.store === currentScope.current.store &&
      destination.resources === currentScope.current.resources,
    [],
  )
  const conversationsOf = (destination: AttachmentDestination) =>
    destination.store.getState().conversation.conversations
  const inputRef = React.useRef<HTMLInputElement>(null)
  const operationsRef = React.useRef(new Set<AttachmentOperation>())
  const [operations, setOperations] = React.useState<ReadonlySet<AttachmentOperation>>(
    new Set(),
  )
  /**
   * Files the host is making readable: its identity for each one, and what to
   * put on the tile.
   *
   * **Keyed on the host's identity, never on the name.** Two folders can each
   * hold a `report.pdf`, and a map keyed on the name had one entry for both:
   * the first to settle took the other's tile away, and the draft became
   * sendable while the second was still being fetched.
   *
   * Host readiness events are separate from local read operations and arrive
   * and leave on the host's schedule, one event each way.
   */
  const [readying, setReadying] = React.useState<
    Map<string, { name: string; destination: AttachmentDestination }>
  >(new Map())
  /**
   * Which conversation each host-minted drop belongs to.
   *
   * Recorded when the drop gesture happens, not when the host finally speaks, which can be
   * three quarters of a minute later. Reading the open tab then put the tile
   * over whichever conversation somebody had since switched to: it blocked
   * *that* draft's send for forty-five seconds for a file it was never going
   * to get, while the draft the file was joining showed nothing at all.
   *
   * Accepted batches stay here until their terminal host result is consumed.
   * A fixed admission limit bounds the map without evicting any result that is
   * still in flight.
   */
  // Picker identity and lifetime are retained by their live operation values
  // above; this map owns only host-minted drops.
  const dropDestinations = React.useRef(new Map<string, AttachmentDestination>())
  const mounted = React.useRef(true)
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
    destination: AttachmentDestination
    refusal: AttachmentRefusal
  } | null>(null)
  const [viewed, setViewed] = React.useState<{
    destination: AttachmentDestination
    placementId: string
  } | null>(null)
  /**
   * Say why something was not taken. Defaults to the active conversation; a
   * folder walk that finishes after somebody has moved on names the one it was
   * dropped into, so its refusal is not shown above another draft quoting
   * another draft's bounds.
   *
   * Replaces whatever was there. There is one of these at a time, and it is
   * always the most recent thing the panel turned away.
   */
  const refuse = React.useCallback(
    (
      refusal: AttachmentRefusal,
      target: AttachmentDestination | string = currentDestination(),
    ) => {
      const destination = typeof target === "string" ? currentDestination(target) : target
      if (
        !destinationIsCurrent(destination) ||
        destination.conversationId !== activeId.current
      )
        return
      setRefused({ destination, refusal })
    },
    [currentDestination, destinationIsCurrent],
  )
  React.useEffect(() => {
    const operations = operationsRef.current
    const drops = dropDestinations.current
    mounted.current = true
    return () => {
      mounted.current = false
      for (const operation of operations) operation.controller?.abort()
      operations.clear()
      drops.clear()
    }
  }, [])
  /**
   * The conversation a batch belongs to. The host's identity for a waiting
   * file begins with its batch, so the tile finds its draft through the
   * gesture that started it rather than through whatever is on screen now.
   *
   * A batch nobody recorded has no destination. The active conversation is
   * not evidence of where a delayed host event began.
   */
  const destinationForBatch = React.useCallback(
    (batch: string) =>
      [...operationsRef.current].find((operation) => operation.pickerRequestId === batch)
        ?.destination ?? dropDestinations.current.get(batch),
    [],
  )
  const consumeDropDestination = React.useCallback((batch: string) => {
    const destination = dropDestinations.current.get(batch)
    if (destination) dropDestinations.current.delete(batch)
    return destination
  }, [])
  React.useEffect(() => {
    let live = true
    // The gesture's own moment, for both gestures. Everything after it can be
    // three quarters of a minute later, and the open tab is no longer evidence
    // of anything by then.
    const batch = onAttachmentBatch((named, gesture) => {
      if (!live) return
      // A picker identity was registered before its command invocation, and
      // its event must not replace that captured owner. Drop identities are
      // host-minted, so their destination is captured at this event.
      const held = dropDestinations.current
      if (gesture === "picked" || held.has(named)) return
      if (held.size >= MAX_ACTIVE_DROP_BATCHES) {
        refuse({ reason: "too-many-selections" }, currentDestination())
        return
      }
      held.set(named, currentDestination())
    })
    const subscription = onAttachmentReadying(({ id, name, readying: waiting }) => {
      if (!live) return
      setReadying((current) => {
        const next = new Map(current)
        // Settling always removes, whatever the outcome was: the sentence for
        // a file that is not coming is the refusal's, and a tile left spinning
        // beside it would be the panel disagreeing with itself.
        if (waiting) {
          const destination = destinationForBatch(id.split(":")[0] ?? "")
          if (!destination) return current
          next.set(id, { name, destination })
        } else next.delete(id)
        return next
      })
    })
    return () => {
      live = false
      void batch.then((stop) => stop())
      void subscription.then((stop) => stop())
    }
  }, [currentDestination, destinationForBatch, refuse, resources, store])
  React.useEffect(() => {
    if (
      viewed &&
      (viewed.destination.store !== store ||
        viewed.destination.resources !== resources ||
        !chat.conversations.some(
          (conversation) =>
            conversation.id === viewed.destination.conversationId &&
            conversation.draft.some(
              (part) => part.type === "file" && part.placementId === viewed.placementId,
            ),
        ))
    )
      setViewed(null)
  }, [chat.conversations, resources, store, viewed])
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
   * A closed tab in the current owner has nobody left to tell, so its notice
   * goes to that owner's active tab. A retired owner must not notify the
   * replacement provider's tab with the same local conversation ID.
   */
  function refuseClosedConversation(destination: AttachmentDestination) {
    if (!destinationIsCurrent(destination)) return
    refuse({ reason: "conversation-closed" }, currentDestination(activeId.current))
  }
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
  const answerRefusal = (destination: AttachmentDestination) =>
    setRefused((current) =>
      current?.destination.store === destination.store &&
      current.destination.resources === destination.resources &&
      current.destination.conversationId === destination.conversationId
        ? null
        : current,
    )
  /** Said only to the conversation it was said about. */
  const refusal =
    refused?.destination.store === store &&
    refused.destination.resources === resources &&
    refused.destination.conversationId === chat.active.id
      ? refused.refusal
      : null
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
    destination: AttachmentDestination,
  ) {
    if (!mounted.current) return
    if (!held.length && !linked.length) return
    const target = conversationsOf(destination).find(
      (conversation) => conversation.id === destination.conversationId,
    )
    // The draft these were for is gone — closed during a fetch that can take
    // three quarters of a minute, or during the read that follows it. There is
    // nothing to attach them to, and saying so on the conversation that no
    // longer exists would be saying it to nobody, so a notice stays in the
    // captured owner and goes to its active tab.
    //
    // It is said at all because the alternative is the silence this module
    // refuses everywhere else: a person who chose five files and got none has
    // been told nothing about any of them. This used to `return` here.
    //
    // This is the authority, and `addChosenFiles` only short-circuits ahead of
    // it: every route into a draft passes here, including the ones that never
    // read a byte.
    if (!target) {
      refuseClosedConversation(destination)
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
      refuse({ reason: "images-not-supported" }, destination)
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
        destination,
      )
      return
    }
    if (targetFiles.length + held.length + linked.length > MAX_DRAFT_ATTACHMENTS) {
      refuse({ reason: "too-many-files" }, destination)
      return
    }
    const carrying = targetFiles
      .filter((file) => !linkedFile(file.resource))
      .map((file) => file.resource)
    if (
      [...carrying, ...held].reduce((total, file) => total + file.size, 0) >
      MAX_DRAFT_ATTACHMENT_BYTES
    ) {
      refuse({ reason: "draft-too-large" }, destination)
      return
    }
    if (
      !destination.resources.canAdd(held.reduce((total, file) => total + file.size, 0))
    ) {
      refuse(
        {
          reason: "window-budget",
          maxMiB: MAX_SESSION_ATTACHMENT_BYTES / MIB,
          draftsHoldFiles: conversationsOf(destination).some((conversation) =>
            conversation.draft.some((part) => part.type === "file"),
          ),
        },
        destination,
      )
      return
    }
    answerRefusal(destination)
    try {
      // Object URLs are ready synchronously; no FileReader or duplicate thumbnail URLs.
      destination.attachFiles(
        [...destination.resources.add(held), ...destination.resources.addChosen(linked)],
        destination.conversationId,
      )
    } catch {
      refuse({ reason: "unreadable-files" }, destination)
    }
  }

  function addFiles(
    selected: readonly File[],
    target: AttachmentDestination | string = currentDestination(),
  ) {
    const destination = typeof target === "string" ? currentDestination(target) : target
    attach(selected, [], destination)
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
  async function chooseFiles(destination = currentDestination()) {
    if (!mounted.current) return
    // The request identity travels with the host's separate readiness events,
    // so a second picker can overlap without borrowing the first one's owner.
    const requestId: PickerRequestId = `p-${crypto.randomUUID()}`
    if (
      [...operationsRef.current].some(
        (operation) => operation.pickerRequestId === requestId,
      )
    ) {
      refuse({ reason: "picker-unavailable" }, destination)
      return
    }
    const pickerOperation: AttachmentOperation = {
      destination,
      files: [],
      pickerRequestId: requestId,
    }
    operationsRef.current.add(pickerOperation)
    if (mounted.current) setOperations(new Set(operationsRef.current))
    let chosen
    try {
      chosen = await chooseAttachmentFiles(requestId)
      if (!operationIsActive(pickerOperation)) return
    } catch (error) {
      if (operationIsActive(pickerOperation)) refuse(pickerRefusal(error), destination)
      return
    } finally {
      finishOperation(pickerOperation)
      if (mounted.current)
        setReadying((current) => {
          const next = new Map(current)
          for (const id of next.keys()) {
            if (id.startsWith(`${requestId}:`)) next.delete(id)
          }
          return next
        })
    }
    // Teardown retires this operation owner. A late answer cannot open the
    // browser picker or admit another operation after that point.
    if (chosen === null) {
      inputRef.current?.click()
      return
    }
    await addChosenFiles(chosen, destination)
  }

  /** Whether a conversation is still open to attach to. */
  const stillOpen = (destination: AttachmentDestination) =>
    conversationsOf(destination).some(
      (conversation) => conversation.id === destination.conversationId,
    )

  function beginPending(
    destination: AttachmentDestination,
    files: { name: string; mimeType: string; previewUrl?: string }[],
    controller?: AbortController,
  ) {
    if (!mounted.current) return null
    if (
      [...operationsRef.current].some(
        (operation) =>
          !operation.pickerRequestId &&
          operation.destination.store === destination.store &&
          operation.destination.resources === destination.resources,
      )
    ) {
      refuse({ reason: "reading-files" }, destination)
      return null
    }
    const operation: AttachmentOperation = {
      destination,
      files,
      ...(controller ? { controller } : {}),
    }
    operationsRef.current.add(operation)
    if (mounted.current) setOperations(new Set(operationsRef.current))
    answerRefusal(destination)
    return operation
  }

  function finishOperation(operation: AttachmentOperation) {
    if (!operationsRef.current.delete(operation)) return
    if (mounted.current) setOperations(new Set(operationsRef.current))
  }

  const operationIsActive = (operation: AttachmentOperation) =>
    mounted.current && operationsRef.current.has(operation)

  /**
   * Attach what the picker chose: read the images, point at everything else.
   *
   * Reading is why this waits. A selection is attached whole or not at all, so
   * one image that will not read turns the whole selection away with its
   * reason rather than leaving a draft short of what was picked.
   */
  async function addChosenFiles(
    chosen: readonly ChosenFile[],
    target: AttachmentDestination | string = currentDestination(),
  ) {
    if (!mounted.current) return
    const destination = typeof target === "string" ? currentDestination(target) : target
    if (!chosen.length) return
    // Checked before the read as well as inside `attach`, which is what makes
    // the common case cheap: a selection can be forty-five seconds behind its
    // gesture, and a draft closed in that time has nothing to read images for.
    // It is a short-circuit, not the authority — a conversation closed *during*
    // the read still reaches `attach`, and the bytes for that one are spent and
    // thrown away. Both end in the same sentence.
    if (!stillOpen(destination)) {
      refuseClosedConversation(destination)
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
      refuse({ reason: "file-not-linkable", name: unusable.name }, destination)
      return
    }
    // A conversation on an SSH host reads that host's files: a path on this
    // machine would name nothing there, or something else. Images carry
    // their bytes and still go.
    const conversation = conversationsOf(destination).find(
      (conversation) => conversation.id === destination.conversationId,
    )
    if (linked.length && conversation && !linksFilesHere(conversation)) {
      refuse({ reason: "file-not-on-host", name: linked[0].name }, destination)
      return
    }
    if (!images.length) {
      attach([], linked, destination)
      return
    }
    const operation = beginPending(
      destination,
      images.map((image) => ({ name: image.name, mimeType: image.mimeType })),
    )
    if (!operation) return
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
      if (!operationIsActive(operation)) return
    } catch (error) {
      if (operationIsActive(operation)) refuse(pickerRefusal(error), destination)
      return
    } finally {
      finishOperation(operation)
    }
    attach(held, linked, destination)
  }

  async function addImageUrl(
    url: string,
    target: AttachmentDestination | string = currentDestination(),
  ) {
    const destination = typeof target === "string" ? currentDestination(target) : target
    const controller = new AbortController()
    const operation = beginPending(
      destination,
      [{ name: "Image", mimeType: "image/" }],
      controller,
    )
    if (!operation) return
    try {
      const file = await readDroppedImage(url, controller.signal, MAX_ATTACHMENT_BYTES)
      if (!operationIsActive(operation)) return
      finishOperation(operation)
      attach([file], [], destination)
    } catch {
      if (operationIsActive(operation))
        refuse({ reason: "unreadable-image-url" }, destination)
    } finally {
      finishOperation(operation)
    }
  }
  return {
    inputRef,
    /**
     * Whether this conversation is waiting on something that has to finish
     * before a draft can go — selected image bytes being read, an image being
     * fetched from a URL, or a file the host is still making readable. The
     * composer refuses a send while any of these is true, with the words it
     * already has for it.
     */
    isPending: (conversationId: string) =>
      [...operations].some(
        (operation) =>
          !operation.pickerRequestId &&
          operation.destination.store === store &&
          operation.destination.resources === resources &&
          operation.destination.conversationId === conversationId,
      ) ||
      [...readying.values()].some(
        (file) =>
          file.destination.store === store &&
          file.destination.resources === resources &&
          file.destination.conversationId === conversationId,
      ),
    /**
     * Tiles for files that are on their way but not attached. Both kinds
     * together, because they are the same thing to look at and the same thing
     * to wait for; neither claims the file is on the draft.
     */
    pendingFiles: [
      ...[...operations]
        .filter(
          (operation) =>
            !operation.pickerRequestId &&
            operation.destination.store === store &&
            operation.destination.resources === resources &&
            operation.destination.conversationId === chat.active.id,
        )
        .flatMap((operation) => operation.files),
      ...[...readying.values()]
        .filter(
          (file) =>
            file.destination.store === store &&
            file.destination.resources === resources &&
            file.destination.conversationId === chat.active.id,
        )
        .map((file) => ({ name: file.name, mimeType: "", previewUrl: undefined })),
    ],
    files,
    reading: [...operations].some(
      (operation) =>
        !operation.pickerRequestId &&
        operation.destination.store === store &&
        operation.destination.resources === resources &&
        operation.destination.conversationId === chat.active.id,
    ),
    refusal,
    refuse: (
      refusal: AttachmentRefusal,
      target: AttachmentDestination | string = currentDestination(),
    ) => refuse(refusal, target),
    /**
     * The draft has gone to the gateway, so whatever was refused before it is
     * over. Called for a submission that was actually taken — a send the panel
     * turned away has not moved anybody on, and is usually itself the refusal
     * being shown.
     */
    draftSent: (id: string) => answerRefusal(currentDestination(id)),
    clearRefusal: () =>
      setRefused((current) =>
        current && destinationIsCurrent(current.destination) ? null : current,
      ),
    addFiles,
    addImageUrl,
    consumeDropDestination,
    captureDestination: currentDestination,
    isCurrent: (destination: AttachmentDestination) =>
      destinationIsCurrent(destination) &&
      destination.conversationId === activeId.current,
    chooseFiles: () => void chooseFiles(currentDestination()),
    addChosenFiles,
    viewed:
      viewed?.destination.store === store &&
      viewed.destination.resources === resources &&
      viewed.destination.conversationId === chat.active.id
        ? (chat.active.draft.find(
            (part): part is FileAttachment =>
              part.type === "file" && part.placementId === viewed.placementId,
          ) ?? null)
        : null,
    open: (file: FileAttachment) =>
      setViewed({
        destination: currentDestination(),
        placementId: file.placementId,
      }),
    close: () => setViewed(null),
    remove: (id: string) => {
      chat.removeFile(id)
      // Taking a tile off is an answer to "attach fewer" and to "this draft is
      // too heavy" alike, whichever of them was being said.
      answerRefusal(currentDestination())
      if (viewed?.placementId === id) setViewed(null)
    },
  }
}
