import {
  parseConversationTabSnapshot,
  type SavedConversationTabs,
} from "../../application/saved-tabs"

type Owner = { gatewayId: string; principalId: string; organizationId: string }
type StoragePort = Pick<Storage, "getItem" | "setItem">
function key(owner: Owner) {
  return `nessa.tabs:${JSON.stringify([owner.gatewayId, owner.organizationId, owner.principalId])}`
}
/** Only authenticated, identity-scoped references persist; messages and credentials never do. */
export function createTabStorage(storage: StoragePort) {
  return {
    read(owner: Owner): SavedConversationTabs | null {
      try {
        const text = storage.getItem(key(owner))
        if (!text || text.length > 65536) return null
        return parseConversationTabSnapshot(JSON.parse(text))
      } catch {
        return null
      }
    },
    write(owner: Owner, tabs: SavedConversationTabs) {
      try {
        storage.setItem(key(owner), JSON.stringify(tabs))
      } catch {
        // Browser storage restrictions must not interrupt a live conversation.
      }
    },
  }
}
