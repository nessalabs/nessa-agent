/**
 * Clock ticks shared by UI surfaces that render labels which age.
 *
 * Conversation and workspace own their labels; this module owns the browser
 * clock subscription contract they both consume. Each provider keeps its
 * injected clock, and each interval publishes a stable revision rather than
 * deriving a React snapshot from time that advances independently.
 */
export { ClockProvider, useNow } from "./ui/provider"
