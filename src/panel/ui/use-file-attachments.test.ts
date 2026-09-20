// @vitest-environment jsdom
/**
 * When a refusal is said, and for how long — over the whole path, because every
 * bug this has had lived in a join rather than in a part.
 *
 * So this is the panel's own composition, minus its chrome: a real
 * `FileDropZone` over a real `useFileAttachments`, a real `useComposer` sending
 * through a real store, and the real `AttachmentNotices` reading what comes
 * out. Files are dropped by dispatching drop events, not by calling `addFiles`,
 * because the zone hands over what passed before what it refused and that
 * ordering is where a refusal was being lost.
 *
 * Three things are settled here and nowhere else, all of them about time. It is
 * said at the moment it happens, including when the same drop attached
 * something. It is not revealed later, when something unrelated finishes. And
 * it stops being said when it has been answered — by attaching, by removing, or
 * by the draft going — and does not come back when a refused send hands the
 * same files back.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

// The conversation barrel also exports its components, which need the whole UI
// package resolved. Its `testing` entry is the same pure functions with no
// component among them, so the barrel is mocked with that: one definition.
vi.mock("../../conversation", () => import("../../conversation/testing"))

import { createDependencies } from "../../composition/dependencies"
import {
  attachFiles,
  openConversation,
  scenarioEffects,
  setActive,
  setDraft,
  SubmissionRefusedError,
  uploadChanged,
  useConversation,
  MAX_ATTACHMENT_BYTES,
  type ConversationEffects,
} from "../../conversation/testing"
import { sessionReady } from "../../session/testing"
import { makeStore } from "../../store"
import type { AttachmentResources } from "../adapters/attachment-resources"
import { AttachmentDropZone } from "./attachment-drop-zone"
import { AttachmentNotices } from "./attachment-notices"
import { useComposer } from "./use-composer"
import { useFileAttachments } from "./use-file-attachments"

/** A File of a given weight without allocating it: only its fields are read. */
const weighing = (name: string, size: number, type = "video/mp4") =>
  ({ name, size, type }) as File

const huge = () => weighing("holiday.mp4", MAX_ATTACHMENT_BYTES + 1)
const photo = () => weighing("photo.png", 1024, "image/png")

/**
 * The resource store, as much of it as this needs. jsdom has no
 * `createObjectURL`, and nothing here reads bytes back; what it must do is hand
 * out a distinct identity per file, the way the real one does, so that a draft
 * restored with the same files is genuinely the same files.
 */
let taken = 0
const resources: AttachmentResources = {
  add: (files) =>
    files.map((file) => ({
      type: "file" as const,
      id: `r${taken++}`,
      name: file.name,
      mimeType: file.type || "application/octet-stream",
      size: file.size,
      previewUrl: "blob:test",
      upload: { status: "not-started" as const },
    })),
  canAdd: () => true,
  bytes: () => undefined,
  retain: () => {},
}

const retryUploads = vi.fn()

/** The panel's own composition of the pieces, and nothing else of the panel. */
function Surface() {
  const chat = useConversation()
  const attachments = useFileAttachments(chat, resources)
  const { submit } = useComposer(chat, () => false, attachments.draftSent)
  return React.createElement(AttachmentDropZone, {
    onFiles: attachments.addFiles,
    onRefused: attachments.refuse,
    children: React.createElement(
      "form",
      { onSubmit: submit },
      React.createElement("div", { "data-panel": true }),
      React.createElement("button", { type: "submit" }, "Send"),
      React.createElement(AttachmentNotices, {
        refusal: attachments.refusal,
        files: attachments.files,
        imageInput: chat.active.remote?.capabilities.imageInput,
        onRetryUploads: retryUploads,
        onChooseFiles: vi.fn(),
        onDismissRefusal: attachments.clearRefusal,
      }),
      React.createElement(
        "button",
        {
          type: "button",
          "data-remove": true,
          onClick: () => {
            const [first] = attachments.files
            if (first) attachments.remove(first.id)
          },
        },
        "Remove",
      ),
    ),
  })
}

let container: HTMLDivElement
let root: Root

const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)
/** Each notice's own live region, in the order they are read down the screen. */
const announced = () =>
  [...container.querySelectorAll("[aria-live]")].map((region) => region.textContent ?? "")

async function mount(effects: Partial<ConversationEffects> = {}) {
  const store = makeStore(
    createDependencies({ conversation: { ...scenarioEffects("echo"), ...effects } }),
  )
  // Connected, as the panel usually is: only the phase and a hello are read.
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  await React.act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface),
      }),
    )
  })
  return store
}

/**
 * Drop files on the zone, as a browser would. jsdom has no `DataTransfer`, so
 * the event carries a stand-in with the three things the zone reads; an empty
 * `items` is what a browser gives when the entry API is unavailable, which is
 * the path a plain file drop takes.
 */
async function drop(files: readonly File[]) {
  const event = new Event("drop", { bubbles: true, cancelable: true })
  Object.defineProperty(event, "dataTransfer", {
    value: { types: ["Files"], files, items: [] },
  })
  await React.act(async () => {
    container.querySelector("[data-panel]")!.dispatchEvent(event)
  })
  await React.act(async () => {})
}

const pressSend = async () => {
  await React.act(async () => {
    container.querySelector<HTMLButtonElement>("button[type=submit]")!.click()
  })
  await React.act(async () => {})
}

const pressRemove = async () => {
  await React.act(async () => {
    container.querySelector<HTMLButtonElement>("[data-remove]")!.click()
  })
}

const draftOf = (store: ReturnType<typeof makeStore>, id = "c0") =>
  store.getState().conversation.conversations.find((item) => item.id === id)!.draft

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // jsdom has no `matchMedia`, and the notification asks it about reduced
  // motion before it plays its exit.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
  vi.clearAllMocks()
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("says why a file was refused even when the same drop attached another", async () => {
  // The mixed drop. The zone hands over what passed and then what it refused,
  // so the refusal is written after the attach and has to survive it — which
  // it did not while what to show was worked out by comparing the draft.
  const store = await mount()
  await drop([photo(), huge()])
  expect(draftOf(store).map((part) => part.type === "file" && part.name)).toEqual([
    "photo.png",
  ])
  expect(announced()).toEqual([expect.stringContaining("File is too large")])
  expect(announced()[0]).toContain("holiday.mp4")
})

it("says a refusal at the moment it happens, without taking a failed upload's Retry away", async () => {
  const store = await mount()
  await drop([photo()])
  const [file] = draftOf(store)
  await React.act(async () => {
    store.dispatch(
      uploadChanged({ fileId: (file as { id: string }).id, to: "uploading" }),
    )
    store.dispatch(
      uploadChanged({
        fileId: (file as { id: string }).id,
        to: "failed",
        reason: "unavailable",
      }),
    )
  })
  expect(announced()).toEqual([expect.stringContaining("Image didn't upload")])

  // The moment the panel used to lose one of the two, whichever way it ranked.
  await drop([huge()])
  const [draft, refusal] = announced()
  expect(draft).toContain("Image didn't upload")
  expect(refusal).toContain("File is too large")
  await React.act(async () => {
    button("Retry")!.click()
  })
  expect(retryUploads).toHaveBeenCalledOnce()
})

it("stops saying a refusal once a later attempt actually attached something", async () => {
  // The attempt the refusal was about has been superseded by one that worked.
  // This is the same clearing the mixed drop above relies on being overridden,
  // so both halves of that ordering are held down: it happens, and the refusal
  // written after it wins.
  await mount()
  await drop([huge()])
  expect(announced()).toEqual([expect.stringContaining("File is too large")])
  await drop([photo()])
  expect(announced()).toEqual([])
})

it("stops saying a refusal once a file is taken off the draft", async () => {
  // "Attach fewer, or send what is here first" is not something to go on saying
  // above a draft somebody has since emptied.
  await mount()
  await drop([photo()])
  await drop(Array.from({ length: 21 }, (_, index) => weighing(`${index}.mp4`, 1)))
  expect(announced()).toEqual([expect.stringContaining("Too many files")])
  // Not the old sentence, which listed all three bounds whichever one broke.
  expect(announced()[0]).not.toContain("64 MiB")

  await pressRemove()
  expect(announced()).toEqual([])
})

it("stops saying a refusal once the draft has gone, even with no files in it", async () => {
  // Nothing was attached, so there is no file change to notice: the send itself
  // is the answer, and saying it is the composer's job rather than something to
  // work out afterwards.
  const store = await mount()
  await drop([huge()])
  expect(announced()).toEqual([expect.stringContaining("File is too large")])
  await React.act(async () => {
    store.dispatch(setDraft({ draft: [{ type: "text", text: "never mind" }] }))
  })
  await pressSend()
  expect(draftOf(store)).toEqual([])
  expect(announced()).toEqual([])
})

it("does not bring a refusal back when a refused send hands the draft over again", async () => {
  // The draft goes, the gateway refuses it, and the draft comes back — the same
  // content, under the identities it had before. A refusal the send had already
  // answered must not come back with it, beside a notice about the send that
  // has nothing to do with it.
  //
  // The draft here is text. Reproducing the report's own sequence, where the
  // restored draft carries the *files* it was sent with, needs the tab's image
  // capabilities, and those arrive from a gateway read this harness cannot get
  // the scenario to answer; that much is not covered here. What is covered is
  // the mechanism behind it, and there is no longer a file-shaped half of it:
  // nothing works out whether to show a refusal by looking at the draft.
  const store = await mount({
    send: () => Promise.reject(new SubmissionRefusedError("attachment-not-found")),
  })
  await drop([huge()])
  expect(announced()).toEqual([expect.stringContaining("File is too large")])
  await React.act(async () => {
    store.dispatch(setDraft({ draft: [{ type: "text", text: "here you go" }] }))
  })

  await pressSend()
  const conversation = store
    .getState()
    .conversation.conversations.find((item) => item.id === "c0")!
  // The send really did leave, really did fail, and really did give the draft
  // back rather than keeping it.
  expect(conversation.error).toBeTruthy()
  expect(conversation.draft).toEqual([{ type: "text", text: "here you go" }])
  expect(
    conversation.turns.some((turn) => turn.from === "user" && turn.receipt === "failed"),
  ).toBe(true)
  // Whatever the conversation has to say about the send, the panel has nothing
  // more to say about a file that was never in the draft.
  expect(announced().join(" ")).not.toContain("File is too large")
})

it("does not say one conversation's refusal above another's composer", async () => {
  // Scoped, because a folder walk can finish after somebody has moved on, and
  // the refusal quotes the bounds of the draft it was dropped into.
  const store = await mount()
  await drop([huge()])
  expect(announced()).toEqual([expect.stringContaining("File is too large")])
  await React.act(async () => {
    store.dispatch(openConversation())
  })
  expect(announced()).toEqual([])
  await React.act(async () => {
    store.dispatch(setActive("c0"))
  })
  expect(announced()).toEqual([expect.stringContaining("File is too large")])
})

it("refuses a file the + picker would refuse too, and does not send anybody to it", async () => {
  // The 720 MB .mp4 from the report. Both routes hold a file to the same bound,
  // which is exactly why the advice to try the other one was wrong.
  await mount()
  await drop([huge()])
  expect(announced()[0]).toContain("64 MiB")
  expect(button("Choose files")).toBeNull()
  expect(button("Retry")).toBeNull()

  await React.act(async () => {
    button("Dismiss notification")!.click()
  })
  expect(announced()).toEqual([])
})

it("keeps a refusal that the conversation, not the panel, turned the send away for", async () => {
  // A send the store refuses leaves the draft where it was, so nothing has been
  // answered and the notice stays. Here the draft holds a file that cannot be
  // sent at all.
  const store = await mount()
  await React.act(async () => {
    store.dispatch(
      attachFiles({
        files: [
          {
            type: "file",
            id: "n",
            name: "notes.pdf",
            mimeType: "application/pdf",
            size: 2,
            previewUrl: "blob:test",
            upload: { status: "not-started" },
          },
        ],
        conversationId: "c0",
      }),
    )
  })
  await drop([huge()])
  expect(announced()).toEqual([
    expect.stringContaining("File can't be sent"),
    expect.stringContaining("File is too large"),
  ])
  await pressSend()
  expect(draftOf(store)).not.toEqual([])
  expect(announced().join(" ")).toContain("File is too large")
})
