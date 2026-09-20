import { describe, expect, it } from "vitest"
import {
  declaredMediaType,
  humanSize,
  imageReferenceLabel,
  isImageFile,
  messageImages,
  messageLabel,
  previewableImage,
  validDraftAttachments,
  validImageReference,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_SEND_IMAGES,
  MAX_SEND_TOTAL_IMAGE_BYTES,
  type FileAttachment,
  type ImageReference,
  type UploadState,
} from "./attachments"
import { contentText, referencedContent, type MessageContent } from "./content"

const DIGEST = `sha256:${"ab".repeat(32)}`
const reference = (change: Partial<ImageReference> = {}): ImageReference => ({
  digest: DIGEST,
  mimeType: "image/png",
  size: 3,
  ...change,
})

function image(
  id: string,
  upload: UploadState = { status: "stored", image: reference() },
  change: Partial<FileAttachment> = {},
): FileAttachment {
  return {
    type: "file",
    id,
    name: `${id}.png`,
    mimeType: "image/png",
    previewUrl: `blob:${id}`,
    size: 3,
    upload,
    ...change,
  }
}

it("accepts empty files and exact file limit, rejects invalid byte sizes", () => {
  const file: FileAttachment = {
    type: "file",
    id: "a",
    name: "a.txt",
    mimeType: "text/plain",
    previewUrl: "blob:test-empty",
    size: 0,
    upload: { status: "not-started" },
  }
  expect(validDraftAttachments([file])).toBe(true)
  expect(validDraftAttachments([{ ...file, size: MAX_ATTACHMENT_BYTES }])).toBe(true)
  for (const size of [-1, 0.5, Number.NaN, Infinity, MAX_ATTACHMENT_BYTES + 1]) {
    expect(validDraftAttachments([{ ...file, size }])).toBe(false)
  }
})

describe("which parts of a message can go as images", () => {
  it("sends text alone as no images", () => {
    expect(messageImages([{ type: "text", text: "hi" }])).toEqual({
      ok: true,
      images: [],
    })
  })

  it("sends what the gateway stored, never anything read off the attached file", () => {
    // A 9 MB HEIC was attached; the gateway stored a 400 KB JPEG under its own digest.
    const stored = reference({
      digest: `sha256:${"cd".repeat(32)}`,
      mimeType: "image/jpeg",
      size: 400_000,
    })
    const heic = image(
      "holiday",
      { status: "stored", image: stored },
      { name: "holiday.heic", mimeType: "image/heic", size: 9_000_000 },
    )
    expect(messageImages([{ type: "text", text: "look" }, image("a"), heic])).toEqual({
      ok: true,
      images: [reference(), stored],
    })
  })

  it("treats any image/* file as an image, and nothing else", () => {
    for (const type of ["image/png", "image/heic", "image/tiff", "image/svg+xml"])
      expect(isImageFile(type)).toBe(true)
    for (const type of ["application/pdf", "text/plain", "application/octet-stream", ""])
      expect(isImageFile(type)).toBe(false)
  })

  it("counts the message budgets over stored references, not over attached files", () => {
    // What those budgets are is the protocol's; the gateway adapter, which sees
    // both, is where they are held to the generated bound.
    // Three 18 MB originals the gateway brought down to 1 MB each: they fit.
    const shrunk = Array.from({ length: 3 }, (_, index) =>
      image(
        `big${index}`,
        { status: "stored", image: reference({ size: 1024 * 1024 }) },
        { size: 18 * 1024 * 1024 },
      ),
    )
    expect(messageImages(shrunk).ok).toBe(true)
    // Three tiny originals stored at 4 MB each do not, whatever the files weigh.
    const grown = Array.from({ length: 3 }, (_, index) =>
      image(`small${index}`, {
        status: "stored",
        image: reference({ size: 4 * 1024 * 1024 }),
      }),
    )
    expect(messageImages(grown)).toEqual({
      ok: false,
      refusal: { kind: "images-too-large" },
    })
  })

  it("puts no per-image byte limit of its own on a stored reference", () => {
    // How heavy one image may be is the gateway's rule. Only the message total is ours.
    const heavy = image("heavy", {
      status: "stored",
      image: reference({ size: MAX_SEND_TOTAL_IMAGE_BYTES }),
    })
    expect(messageImages([heavy]).ok).toBe(true)
  })

  it.each<[string, MessageContent, object]>([
    [
      "a file that is not an image",
      [
        image("a"),
        image("doc", undefined, { name: "notes.pdf", mimeType: "application/pdf" }),
      ],
      { kind: "unsupported-file", name: "notes.pdf" },
    ],
    [
      "a failed upload",
      [
        image("a", { status: "uploading" }),
        image("b", { status: "failed", reason: "unsupported-image" }),
      ],
      { kind: "upload-failed", name: "b.png" },
    ],
    [
      "an upload in flight",
      [image("a", { status: "uploading" })],
      { kind: "upload-in-flight" },
    ],
    [
      "an upload not started",
      [image("a", { status: "not-started" })],
      { kind: "upload-in-flight" },
    ],
    [
      "a stored reference whose digest is not one",
      [image("a", { status: "stored", image: reference({ digest: "sha256:ABC" }) })],
      { kind: "upload-failed", name: "a.png" },
    ],
    [
      "a stored reference in an encoding no message names",
      [
        image("a", {
          status: "stored",
          image: reference({ mimeType: "image/heic" as "image/png" }),
        }),
      ],
      { kind: "upload-failed", name: "a.png" },
    ],
    [
      "a stored reference to nothing",
      [image("a", { status: "stored", image: reference({ size: 0 }) })],
      { kind: "upload-failed", name: "a.png" },
    ],
    [
      "more than ten images",
      Array.from({ length: MAX_SEND_IMAGES + 1 }, (_, index) => image(`i${index}`)),
      { kind: "too-many-images" },
    ],
  ])("refuses %s, whole", (_name, content, refusal) => {
    expect(messageImages(content)).toEqual({ ok: false, refusal })
  })

  it("accepts exactly ten images weighing exactly 10 MiB", () => {
    const content = Array.from({ length: MAX_SEND_IMAGES }, (_, index) =>
      image(`i${index}`, {
        status: "stored",
        image: reference({ size: 1024 * 1024 }),
      }),
    )
    const result = messageImages(content)
    expect(result.ok && result.images).toHaveLength(10)
  })

  it("knows a reference from something shaped like one", () => {
    expect(validImageReference(reference())).toBe(true)
    for (const bad of [
      { digest: "ab".repeat(32) },
      { digest: DIGEST.toUpperCase() },
      { mimeType: "image/svg+xml" },
      { size: 0 },
      { size: 1.5 },
      { size: "3" },
    ])
      expect(validImageReference({ ...reference(), ...bad })).toBe(false)
  })
})

describe("content read back by reference", () => {
  it("builds text then reference-only image parts, which carry no text", () => {
    const content = referencedContent("caption", [
      reference({ mimeType: "image/jpeg", size: 2048 }),
    ])
    expect(content).toEqual([
      { type: "text", text: "caption" },
      { type: "image-reference", digest: DIGEST, mimeType: "image/jpeg", size: 2048 },
    ])
    expect(contentText(content)).toBe("caption")
    // A reference is already what a message sends; it needs no upload to go again.
    expect(messageImages(content)).toEqual({
      ok: true,
      images: [reference({ mimeType: "image/jpeg", size: 2048 })],
    })
    expect(referencedContent("", [])).toEqual([])
  })

  it("labels what it cannot show, and names a message that has no text", () => {
    // Binary units, named as binary units: 812 * 1024 bytes is 812 KiB, not 812 KB.
    expect(imageReferenceLabel("image/webp", 812 * 1024)).toBe("WebP image, 812 KiB")
    expect(humanSize(512)).toBe("512 B")
    expect(humanSize(3.4 * 1024 * 1024)).toBe("3.4 MiB")
    expect(messageLabel("  hello ", 2, "a.png")).toBe("hello")
    expect(messageLabel(" ", 1, "finder.png")).toBe("finder.png")
    expect(messageLabel("", 1)).toBe("1 image")
    expect(messageLabel("", 3, "a.png")).toBe("3 images")
    expect(messageLabel("", 0)).toBe("")
  })
})

describe("a file the browser could not name", () => {
  it.each([
    ["holiday.heic", "image/heic"],
    ["holiday.HEIF", "image/heif"],
    ["scan.avif", "image/avif"],
    ["render.jxl", "image/jxl"],
    ["poster.psd", "image/vnd.adobe.photoshop"],
    ["scan.tif", "image/tiff"],
    ["scan.tiff", "image/tiff"],
    ["icon.bmp", "image/bmp"],
    ["IMG_0001.DNG", "image/x-adobe-dng"],
    ["IMG_0002.cr2", "image/x-canon-cr2"],
    ["IMG_0003.CR3", "image/x-canon-cr3"],
    ["DSC_0004.nef", "image/x-nikon-nef"],
    ["DSC0005.arw", "image/x-sony-arw"],
    ["DSCF0006.raf", "image/x-fuji-raf"],
    ["P0000007.orf", "image/x-olympus-orf"],
    ["P0000008.rw2", "image/x-panasonic-rw2"],
    ["IMGP0009.pef", "image/x-pentax-pef"],
    ["SAM_0010.srw", "image/x-samsung-srw"],
  ])("declares %s as %s, which makes it an image to upload", (name, type) => {
    for (const reported of ["", "application/octet-stream", " "]) {
      expect(declaredMediaType(name, reported)).toBe(type)
      expect(isImageFile(declaredMediaType(name, reported))).toBe(true)
    }
    // Every declared type is one the protocol's media-type token pattern accepts.
    expect(type).toMatch(/^[a-z0-9][a-z0-9!#$&^_.+-]*\/[a-z0-9][a-z0-9!#$&^_.+-]*$/)
  })

  it("believes the browser whenever it did give a type", () => {
    expect(declaredMediaType("holiday.heic", "image/jpeg")).toBe("image/jpeg")
    expect(declaredMediaType("notes.cr3", "text/plain")).toBe("text/plain")
    expect(declaredMediaType("photo.PNG", "IMAGE/PNG")).toBe("image/png")
  })

  it("does not call something an image on the strength of an unknown or missing extension", () => {
    for (const name of [
      "notes.bin",
      "archive.tar.gz",
      "README",
      "heic",
      ".",
      "trailing.",
      "",
    ])
      expect(declaredMediaType(name, "")).toBe("application/octet-stream")
  })

  it("knows which images a webview can paint, so the rest get a labelled tile", () => {
    for (const type of [
      "image/png",
      "image/jpeg",
      "image/gif",
      "image/webp",
      "image/svg+xml",
    ])
      expect(previewableImage(type)).toBe(true)
    for (const type of [
      "image/heic",
      "image/x-canon-cr3",
      "image/tiff",
      "application/pdf",
    ])
      expect(previewableImage(type)).toBe(false)
  })

  it("lets a camera RAW file be attached at all: one file's worth of a draft's budget", () => {
    // A single file may be as heavy as the upload path takes, and a draft holds
    // two of those. The file bound itself is the protocol's, checked there.
    expect(MAX_DRAFT_ATTACHMENT_BYTES).toBe(2 * MAX_ATTACHMENT_BYTES)
  })
})
