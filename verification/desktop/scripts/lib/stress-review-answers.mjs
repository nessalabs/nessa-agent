import { tsImport } from "tsx/esm/api"

const { parseWireMessage } = await tsImport(
  "../../../../packages/nessa-client/src/protocol/validate.ts",
  import.meta.url,
)
const { hasUniqueObjectKeys } = await tsImport(
  "../../../../packages/nessa-client/src/protocol/unique-json.ts",
  import.meta.url,
)

const { conversationMutation } = await tsImport(
  "../../../../packages/nessa-client/src/protocol/conversation-validate.ts",
  import.meta.url,
)

/** The bounded wait and final receipt must agree before disappearance can count. */
export function reviewAnswerAcknowledged(receipt, acknowledgedInTime) {
  return acknowledgedInTime && receipt.outcome === "acknowledged"
}

const { bounds, ConversationPermissionSelectionState } = await tsImport(
  "../../../../packages/nessa-client/src/generated/product.ts",
  import.meta.url,
)

/** Observe existing window commands; an ACK does not prove provider settlement. */
export function observeReviewAnswers(page, endpoint, now = Date.now) {
  return observeCommands(page, endpoint, now, {
    method: "conversation.answer",
    fields: ["conversationId", "executionId", "review", "permissionId", "optionId"],
    applied: (payload, requestId) => conversationMutation(payload, requestId).applied,
  })
}

/** Only an exact refusal can satisfy the Ask-only scenario, never a success. */
export function observeAccessRefusals(page, endpoint, now = Date.now) {
  return observeCommands(page, endpoint, now, {
    method: "conversation.setApprovalMode",
    fields: ["conversationId", "mode"],
    applied: () => false,
  })
}

function observeCommands(page, endpoint, now, { method, fields, applied }) {
  const url = new URL("/session", endpoint).href
  const sockets = new Map()
  let active
  const parse = ({ payload }, response = false) => {
    try {
      if (typeof payload !== "string") return null
      const raw = payload
      if (response && Buffer.byteLength(raw, "utf8") > bounds.maxOrdinaryResponseBytes)
        return null
      if (response) return parseWireMessage(raw)
      return hasUniqueObjectKeys(raw) ? JSON.parse(raw) : null
    } catch {
      return null
    }
  }
  const onSocket = (socket) => {
    if (socket.url() !== url) return
    const sent = (event) => {
      const frame = parse(event)
      if (
        !active ||
        frame?.type !== "req" ||
        frame.method !== method ||
        typeof frame.id !== "string" ||
        typeof frame.params?.requestId !== "string" ||
        !frame.params.requestId ||
        !fields.every((field) => frame.params?.[field] === active.target[field])
      )
        return
      if (active.command) {
        active.outcome = "multiple commands"
        return
      }
      active.command = { socket, id: frame.id, requestId: frame.params.requestId }
      active.sentAt = now()
      active.outcome = "unconfirmed"
    }
    const received = (event) => {
      const frame = parse(event, true)
      const command = active?.command
      if (
        !command ||
        command.socket !== socket ||
        frame?.type !== "res" ||
        frame.id !== command.id
      )
        return
      if (active.outcome !== "unconfirmed") return
      active.respondedAt = now()
      if (frame.ok === false) {
        active.outcome = "refused"
        if (typeof frame.error?.code === "string") active.code = frame.error.code
        const selection = frame.error?.details?.selectionState
        if (Object.values(ConversationPermissionSelectionState).includes(selection))
          active.selectionState = selection
      } else {
        try {
          active.outcome = applied(frame.payload, command.requestId)
            ? "acknowledged"
            : "invalid acknowledgement"
        } catch {
          active.outcome = "invalid acknowledgement"
        }
      }
    }
    const close = () => {
      socket.off("framesent", sent)
      socket.off("framereceived", received)
      socket.off("close", close)
      sockets.delete(socket)
    }
    sockets.set(socket, close)
    socket.on("framesent", sent)
    socket.on("framereceived", received)
    socket.once("close", close)
  }
  page.on("websocket", onSocket)
  return {
    begin(target) {
      if (active) throw new Error("a review answer observation is already active")
      const scope = { target: { ...target }, outcome: "not sent", clickAttemptAt: now() }
      active = scope
      return {
        acknowledged: () => scope.outcome === "acknowledged",
        refused: () => scope.outcome === "refused",
        finish() {
          if (active === scope) active = undefined
          return {
            ...scope.target,
            ...(scope.command ? { requestId: scope.command.requestId } : {}),
            outcome: scope.outcome,
            clickAttemptAt: scope.clickAttemptAt,
            ...(scope.sentAt !== undefined ? { sentAt: scope.sentAt } : {}),
            ...(scope.respondedAt !== undefined
              ? { respondedAt: scope.respondedAt }
              : {}),
            ...(scope.selectionState ? { selectionState: scope.selectionState } : {}),
            ...(scope.code ? { code: scope.code } : {}),
          }
        },
      }
    },
    dispose() {
      page.off("websocket", onSocket)
      for (const close of [...sockets.values()]) close()
      active = undefined
    },
  }
}
