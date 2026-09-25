import { describe, expect, it } from "vitest"
import {
  declaredMediaType,
  humanSize,
  imageReferenceLabel,
  isImageFile,
  linkablePath,
  linkedFile,
  messageFiles,
  messageImages,
  messageLabel,
  previewableImage,
  validDraftAttachments,
  validImageReference,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_FILE_PATH_BYTES,
  MAX_SEND_FILES,
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
    path: null,
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
    path: null,
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

  it("never hands back something inherited instead of a media type", () => {
    // Every object inherits `constructor`, `toString` and the rest, so a bare
    // index into the extension table returned a function for these names and
    // threw on the first `startsWith`. The picker route depends on this table,
    // so a file could crash the composer by being called the wrong thing.
    for (const name of [
      "photo.constructor",
      "photo.toString",
      "photo.valueOf",
      "photo.hasOwnProperty",
      "photo.__proto__",
    ]) {
      const declared = declaredMediaType(name, "")
      expect(typeof declared).toBe("string")
      expect(declared).toBe("application/octet-stream")
      expect(isImageFile(declared)).toBe(false)
    }
  })

  it("classifies a file the same way whether the platform typed it or a drop did", () => {
    // The class the earlier test could not see, because it used only `.png`
    // and `.heic` — both in the extension table, so both routes agreed by
    // accident. These are image formats the platform knows and the table does
    // not, and they used to split: dropped they uploaded, picked they became a
    // path behind a per-read approval and a different bound.
    //
    // They agree now because neither route consults the table first. Both ask
    // the platform and fall back to the table, which is one call with one set
    // of inputs, so there is no longer a second place for them to disagree.
    const platformTypes: Record<string, string> = {
      "icon.ico": "image/vnd.microsoft.icon",
      "photo.jpe": "image/jpeg",
      "diagram.svgz": "image/svg+xml",
      "scan.jp2": "image/jp2",
      "scan.jpf": "image/jpx",
      "cursor.xbm": "image/x-xbitmap",
      "art.tga": "image/x-tga",
      "old.dib": "image/bmp",
    }
    for (const [name, platform] of Object.entries(platformTypes)) {
      // The drop route: the browser supplies the type.
      const dropped = declaredMediaType(name, platform)
      // The picker route: the host supplies the same type for the same path.
      const picked = declaredMediaType(name, platform)
      expect([name, dropped]).toEqual([name, picked])
      expect(isImageFile(dropped)).toBe(true)
      // And the table alone — what the picker used to have — does not know
      // any of them, which is why it can never be the first question.
      expect(isImageFile(declaredMediaType(name, ""))).toBe(false)
    }
  })

  it("keeps the extension table as the fallback it is, not a second opinion", () => {
    // A platform that knows nothing still gets the table's answer, which is
    // the case it exists for: a browser reports most RAW files as nothing.
    expect(declaredMediaType("holiday.cr3", "")).toBe("image/x-canon-cr3")
    // And a platform answer always wins, even one the table would disagree
    // with, because the platform looked at the file and the table read a name.
    expect(declaredMediaType("holiday.cr3", "video/mp4")).toBe("video/mp4")
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
    expect(messageLabel("  hello ", 2)).toBe("hello")
    expect(messageLabel(" ", 1)).toBe("1 image")
    expect(messageLabel("", 3)).toBe("3 images")
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

describe("files a message points at", () => {
  const document = (id: string, path: string | null) =>
    image(
      id,
      { status: "not-started" },
      { name: `${id}.pdf`, mimeType: "application/pdf", path },
    )

  it("sends a file by path when the host said where it is, and refuses one when nobody could", () => {
    const linked = document("report", "/Users/ada/report.pdf")
    expect(linkedFile(linked)).toBe(true)
    expect(messageFiles([linked])).toEqual([{ path: "/Users/ada/report.pdf" }])
    // Nothing was uploaded for it, and it is not an image, so the message's
    // image rules have nothing to say about it.
    expect(messageImages([linked])).toEqual({ ok: true, images: [] })

    // The same file from a browser: no path, nothing to send, and the refusal
    // names it rather than leaving it silently behind.
    const unnameable = document("report", null)
    expect(linkedFile(unnameable)).toBe(false)
    expect(messageFiles([unnameable])).toEqual([])
    expect(messageImages([unnameable])).toEqual({
      ok: false,
      refusal: { kind: "unsupported-file", name: "report.pdf" },
    })
  })

  it("keeps a linked file out of every image rule, including the ones about waiting", () => {
    // An upload still in flight holds the whole message back. A linked file is
    // never uploading, so it never does — the two must not be confused.
    const waiting = image("a", { status: "uploading" })
    expect(messageImages([waiting, document("d", "/a.pdf")])).toEqual({
      ok: false,
      refusal: { kind: "upload-in-flight" },
    })
    expect(messageImages([document("d", "/a.pdf")])).toEqual({ ok: true, images: [] })

    // An image whose path is known is still carried as an image: a path would
    // leave it to the model to decide whether to look at it.
    const withPath = image("photo", undefined, { path: "/Users/ada/photo.png" })
    expect(linkedFile(withPath)).toBe(false)
    expect(messageFiles([withPath])).toEqual([])
    expect(messageImages([withPath]).ok).toBe(true)
  })

  it("keeps attachment order and bounds how many files one message points at", () => {
    const content: MessageContent = [
      document("b", "/b.pdf"),
      document("a", "/a.pdf"),
      document("c", "/c.pdf"),
    ]
    expect(messageFiles(content)).toEqual([
      { path: "/b.pdf" },
      { path: "/a.pdf" },
      { path: "/c.pdf" },
    ])

    const many = (count: number): MessageContent =>
      Array.from({ length: count }, (_, index) => document(`f${index}`, `/f${index}.pdf`))
    expect(messageImages(many(MAX_SEND_FILES)).ok).toBe(true)
    expect(messageImages(many(MAX_SEND_FILES + 1))).toEqual({
      ok: false,
      refusal: { kind: "too-many-files" },
    })
  })
})

describe("which paths can be carried to the agent", () => {
  it("accepts the ordinary names a picker really hands over", () => {
    for (const path of [
      "/a",
      "/Users/ada/report.pdf",
      "/Users/ada/report (final) 100%.pdf",
      `/Users/ada/it's a "quoted" name.pdf`,
      "/Users/ada/2026-09-20 10:30.txt",
      "/Users/ada/\u043e\u0442\u0447\u0451\u0442.pdf",
      "/Users/ada/.zshrc",
      "/Users/ada/...",
      "/Users/ada/emoji \u{1f600}.pdf",
      `/${"a".repeat(MAX_FILE_PATH_BYTES - 1)}`,
    ])
      expect(linkablePath(path)).toBe(true)
  })

  it("counts the length in bytes, which is the only unit the gateway counts", () => {
    // Three units were in play and only one of them is the rule. `maxLength`
    // in the schema counts code points, `path.length` here counted UTF-16 code
    // units, and the gateway counts UTF-8 bytes. These two paths have the same
    // number of characters and the same `.length`; only one of them fits, and
    // the other used to be accepted here and refused after it was sent.
    const ascii = `/${"a".repeat(MAX_FILE_PATH_BYTES - 1)}`
    const multibyte = `/${"\u3042".repeat(MAX_FILE_PATH_BYTES - 1)}`
    expect([...ascii].length).toBe([...multibyte].length)
    expect(ascii.length).toBe(multibyte.length)
    expect(new TextEncoder().encode(multibyte).length).toBeGreaterThan(
      MAX_FILE_PATH_BYTES,
    )
    expect(linkablePath(ascii)).toBe(true)
    expect(linkablePath(multibyte)).toBe(false)

    // Exactly at the bound, in bytes, with a multibyte character in it.
    const exact = `/\u3042${"a".repeat(MAX_FILE_PATH_BYTES - 4)}`
    expect(new TextEncoder().encode(exact).length).toBe(MAX_FILE_PATH_BYTES)
    expect(linkablePath(exact)).toBe(true)
    expect(linkablePath(`${exact}a`)).toBe(false)
  })

  it("refuses every path the gateway would, so nothing is refused after it is sent", () => {
    for (const path of [
      "",
      "report.pdf",
      "./report.pdf",
      "~/report.pdf",
      // Control characters, C0 and C1 alike. C1 is the one the published
      // pattern used to let through while the gateway refused it.
      "/Users/ada/a\nb.pdf",
      "/Users/ada/a\u0000b.pdf",
      "/Users/ada/a\u0085b.pdf",
      "/Users/ada/a\u009fb.pdf",
      "/Users/ada/a\u007fb.pdf",
      // Components that do not survive being written as a URI.
      "/",
      "/Users/ada/",
      "//Users/ada/report.pdf",
      "/Users//ada/report.pdf",
      "/Users/ada/.",
      "/Users/ada/..",
      "/Users/../etc/passwd",
      "/Users/./ada/report.pdf",
      `/${"a".repeat(MAX_FILE_PATH_BYTES)}`,
    ])
      expect(linkablePath(path)).toBe(false)
  })

  it("carries a path holding the characters a markdown link is made of", () => {
    // These were refused, because the gateway hands the path to the agent
    // inside `[@name](uri)` and a bracket could close it. That rule named one
    // of the three characters that can — `)` ends a destination and `\`
    // escapes whatever follows it — and both of the others were found later,
    // in production code, by someone else. It is gone rather than extended a
    // third time: the gateway's adapter now percent-encodes the URI down to an
    // allowlist and escapes every ASCII punctuation character in the label, so
    // nothing here depends on what a person may call a file.
    for (const path of [
      "/Users/ada/[draft] notes.pdf",
      "/Users/ada/a]b.pdf",
      "/Users/ada/](file:/etc/passwd) [x/report.pdf",
      "/Users/ada/back\\slash.pdf",
      "/Users/ada/trailing\\",
      "/Users/ada/report).pdf",
      "/Users/ada/`code`.pdf",
      "/Users/ada/<b>bold</b>.pdf",
    ])
      expect(linkablePath(path), path).toBe(true)
  })
})
