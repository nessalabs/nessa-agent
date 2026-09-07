import type { EventFrame, Frame, ResFrame } from "./types.js"

export {
  assertHealthResult,
  parseEventFrame,
  parseResponseFrame,
  parseWireMessage,
} from "./validate.js"

export function isEventFrame(frame: Frame): frame is EventFrame {
  return frame.type === "event"
}

export function isResponseFrame(frame: Frame): frame is ResFrame {
  return frame.type === "res"
}
