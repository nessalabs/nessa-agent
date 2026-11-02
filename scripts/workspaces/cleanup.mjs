/** Allocation intents without a matching owned VM retain unresolved cleanup. */
export function assignmentEvidence(history) {
  const transitions = history.transitions ?? []
  const owned = transitions.filter((entry) => entry.state === "assigned")
  const last = transitions.at(-1)
  return {
    owned,
    unresolved:
      !last ||
      !["assigned", "refused"].includes(last.state) ||
      !owned.some((entry) => entry.vm === last.vm),
    thread: last?.thread,
    vm: last?.vm,
  }
}

/** Stop allocation work before reading its final receipts. The gateway owns
 * process termination and retains the directory until its callback returns. */
export async function reconcileAssignments({
  client,
  gateway,
  conversations,
  assignments,
  unconfirmed,
}) {
  if (client) {
    for (const id of conversations) {
      try {
        await client.conversation.close(id)
      } catch {
        unconfirmed({ thread: id, result: "close unconfirmed" })
      }
    }
    try {
      client.close()
    } catch {
      unconfirmed({ result: "client transport close unconfirmed" })
    }
  }
  if (gateway) {
    await gateway.stop((status) => {
      if (!status.stopped || status.exitCode !== 0)
        unconfirmed({ result: "gateway shutdown unconfirmed", ...status })
      assignments()
    })
  } else assignments()
}
