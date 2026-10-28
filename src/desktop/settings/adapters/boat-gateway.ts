import type {
  BoatApi,
  BoatStatus,
  BoatRuntimeReadParams,
  BoatRuntimeSetParams,
  BoatRuntimeSettings,
} from "@nessa/client"
/** Narrow settings dependency. Tokens are passed directly to the authenticated
 * client and are never retained in the workspace store or browser storage. */
export interface BoatGateway {
  read(): Promise<BoatStatus>
  save(token: string): Promise<BoatStatus>
  disconnect(): Promise<BoatStatus>
  runtimeSettings(params?: BoatRuntimeReadParams): Promise<BoatRuntimeSettings>
  setRuntimeSettings(params: BoatRuntimeSetParams): Promise<BoatRuntimeSettings>
}
export function boatGateway(
  connected: () => Promise<{ readonly boat: BoatApi }>,
): BoatGateway {
  return {
    read: async () => (await connected()).boat.status(),
    save: async (token) => (await connected()).boat.save({ token }),
    disconnect: async () => (await connected()).boat.disconnect(),
    runtimeSettings: async (params) => (await connected()).boat.runtimeSettings(params),
    setRuntimeSettings: async (params) =>
      (await connected()).boat.setRuntimeSettings(params),
  }
}
