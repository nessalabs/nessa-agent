/**
 * The clouds Nessa makes thread VMs on, in the one list every picker reads:
 * the New channel and channel settings places, Devices and Add device, and
 * Settings' cloud for new thread VMs. A cloud the gateway adds to its
 * `Cloud` list gets an entry here, and TypeScript refuses the build until it
 * has one; its connection reader goes in `cloud-states.ts` beside it. Pure.
 */
import type { SettingId } from "../../settings/model/settings-catalogue"

/**
 * The gateway's word for a cloud (the client's `Cloud`; `cloud-states.ts`
 * fails the build when the two differ).
 */
export type Cloud = "boat" | "agent37"

export interface CloudProvider {
  /** The gateway's word for it, also its channel place word. */
  readonly id: Cloud
  readonly name: string
  /** What it is, under its name on a tile. */
  readonly kind: string
  /** One line on what it runs on, for the pages that offer it. */
  readonly detail: string
  /** Its row in Settings, where it is connected. */
  readonly setting: SettingId
}

const providers = {
  boat: {
    id: "boat",
    name: "Boat",
    kind: "Cloud VMs",
    detail: "Cloud workspaces using your connected agent subscriptions.",
    setting: "boat",
  },
  agent37: {
    id: "agent37",
    name: "Agent37",
    kind: "Cloud VMs",
    detail: "Cloud workspaces on your own Agent37 account.",
    setting: "agent37",
  },
} as const satisfies { readonly [id in Cloud]: CloudProvider & { readonly id: id } }

/** Every cloud, in the order the pickers offer them. */
export const clouds: readonly CloudProvider[] = Object.values(providers)

/** Whether `word` names a cloud: a channel place on one of its VMs. */
export function isCloud(word: string): word is Cloud {
  return Object.hasOwn(providers, word)
}

/** The cloud `id` names. */
export function cloudProvider(id: Cloud): CloudProvider {
  return providers[id]
}

/** A cloud as a place to pick, offered only once it is connected. */
export interface CloudChoice {
  readonly id: Cloud
  readonly label: string
  readonly disabled: boolean
}

/**
 * Every cloud as a place to pick: one not connected is listed and disabled,
 * since agents placed there could not start, and says so where `say` asks.
 */
export function cloudChoices(
  connected: (id: Cloud) => boolean,
  say: "named" | "with-state" = "named",
): readonly CloudChoice[] {
  return clouds.map(({ id, name }) => {
    const ready = connected(id)
    return {
      id,
      label: ready || say === "named" ? name : `${name} (not connected)`,
      disabled: !ready,
    }
  })
}
