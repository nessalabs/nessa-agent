/**
 * The digest an upload is identified by: `sha256:` and 64 lowercase hex digits.
 *
 * Reads the whole Blob into memory once. That is bounded by what may be
 * attached at all — one file is at most the 20 MB upload cap — and the copy is
 * released as soon as the hash is taken. It is not the name a message uses: the
 * gateway answers an upload with the reference it stored, and that is.
 */
export async function sha256Digest(bytes: Blob): Promise<string> {
  const hash = new Uint8Array(
    await crypto.subtle.digest("SHA-256", await bytes.arrayBuffer()),
  )
  return `sha256:${Array.from(hash, (byte) => byte.toString(16).padStart(2, "0")).join("")}`
}
