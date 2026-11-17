/** The enrollment command shown by Devices. JSON keeps the code as data.
 * Encoding outside printable ASCII (and the outer quote) keeps one single-quoted
 * argument in POSIX shells, including off-schema gateway replies.
 * gateway-pairing.test.ts checks the representation boundary and exact payload.
 */
export function gatewayEnrollmentCommand(port: number, code: string): string {
  const params = JSON.stringify({ address: `GATEWAY_IP:${port}`, code }).replace(
    /[^\x20-\x26\x28-\x7e]/g,
    (unit) => `\\u${unit.charCodeAt(0).toString(16).padStart(4, "0")}`,
  )
  return `nessa call peer.enroll --params '${params}'`
}
