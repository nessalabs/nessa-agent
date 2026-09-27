/** The side, in pixels, a picture is shrunk to before its colours are read. */
const sampleSide = 48

/**
 * A picture's pixels as RGBA bytes, shrunk to at most `sampleSide` on its
 * longer side: plenty to find its main colours, and quick for any size of
 * picture. A GIF gives its first frame. Rejects when the webview cannot decode
 * the picture.
 */
export async function readImagePixels(image: Blob): Promise<Uint8ClampedArray> {
  const bitmap = await createImageBitmap(image)
  try {
    const scale = Math.min(1, sampleSide / Math.max(bitmap.width, bitmap.height))
    const width = Math.max(1, Math.round(bitmap.width * scale))
    const height = Math.max(1, Math.round(bitmap.height * scale))
    const canvas = document.createElement("canvas")
    canvas.width = width
    canvas.height = height
    const context = canvas.getContext("2d", { willReadFrequently: true })
    if (!context) throw new Error("no 2D canvas")
    context.drawImage(bitmap, 0, 0, width, height)
    return context.getImageData(0, 0, width, height).data
  } finally {
    bitmap.close()
  }
}
