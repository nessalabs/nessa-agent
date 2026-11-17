import { expect, it } from "vitest"
import { gatewayEnrollmentCommand } from "./gateway-pairing"

it.each([
  "ABCD-EFGH",
  "';id;#aaa",
  "$(id)`id`",
  "'\"\\\n\r\t\0",
  "💻'汉字",
  "‘’“”\u2028\u2029'",
])("keeps an opaque code inside one quoted JSON argument: %j", (code) => {
  const command = gatewayEnrollmentCommand(47651, code)
  const prefix = "nessa call peer.enroll --params '"
  expect(command.startsWith(prefix)).toBe(true)
  expect(command.endsWith("'")).toBe(true)
  const argument = command.slice(prefix.length, -1)
  expect(argument).not.toContain("'")
  expect(
    [...argument].every(
      (unit) => unit.charCodeAt(0) >= 0x20 && unit.charCodeAt(0) <= 0x7e,
    ),
  ).toBe(true)
  expect(JSON.parse(argument)).toEqual({ address: "GATEWAY_IP:47651", code })
})
