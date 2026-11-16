import type {
  Agent37Api,
  Agent37Status,
  Cloud,
  CloudApi,
  CloudChoice,
} from "@nessa/client"
/** Narrow settings dependency. The key is passed directly to the
 * authenticated client and is never retained in the workspace store or
 * browser storage. */
export interface Agent37Gateway {
  read(): Promise<Agent37Status>
  save(key: string): Promise<Agent37Status>
  disconnect(): Promise<Agent37Status>
  cloud(): Promise<CloudChoice>
  chooseCloud(cloud: Cloud): Promise<CloudChoice>
}
export function agent37Gateway(
  connected: () => Promise<{ readonly agent37: Agent37Api; readonly cloud: CloudApi }>,
): Agent37Gateway {
  return {
    read: async () => (await connected()).agent37.status(),
    save: async (key) => (await connected()).agent37.save({ key }),
    disconnect: async () => (await connected()).agent37.disconnect(),
    cloud: async () => (await connected()).cloud.read(),
    chooseCloud: async (cloud) => (await connected()).cloud.choose(cloud),
  }
}
