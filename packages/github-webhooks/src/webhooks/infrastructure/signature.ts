export interface PayloadVerifier {
  verify(payload: Uint8Array, signature: string | null): Promise<boolean>
  digest(payload: Uint8Array): Promise<string>
}

export function githubPayloadVerifier(
  secret: string,
  subtle: SubtleCrypto,
): PayloadVerifier {
  const key = subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["verify"],
  )
  return {
    async verify(payload, signature) {
      if (signature === null || !/^sha256=[0-9a-f]{64}$/.test(signature)) {
        return false
      }
      const bytes = Uint8Array.from({ length: 32 }, (_, index) =>
        Number.parseInt(signature.slice(7 + index * 2, 9 + index * 2), 16),
      )
      return subtle.verify("HMAC", await key, bytes, payload as Uint8Array<ArrayBuffer>)
    },
    async digest(payload) {
      const result = await subtle.digest("SHA-256", payload as Uint8Array<ArrayBuffer>)
      return Array.from(new Uint8Array(result), (byte) =>
        byte.toString(16).padStart(2, "0"),
      ).join("")
    },
  }
}
