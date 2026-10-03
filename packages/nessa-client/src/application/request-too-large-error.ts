/**
 * A request this client did not send: encoded as a frame, it is longer than
 * the gateway takes (`bounds.maxRequestFrameBytes`, the gateway's own message
 * limit). The gateway would close the socket on it rather than answer, so it
 * is refused here, and nothing reached the gateway.
 */
export class NessaRequestTooLargeError extends Error {
  constructor(
    /** The method that was not sent. */
    readonly method: string,
    /** The encoded frame's length, in UTF-8 bytes. */
    readonly bytes: number,
  ) {
    super(`Request ${method} is ${bytes} bytes, past the gateway's limit`)
    this.name = "NessaRequestTooLargeError"
  }
}
