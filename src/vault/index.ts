/** Shared vault setup/unlock dialog and typed source, reusable by protected features.
 * Application state retains lifecycle only; adapters own native/gateway transport. */
export { VaultProvider, useVaultUnlock } from "./ui/provider"
export { VaultPassphraseControl } from "./ui/change-control"
export { nativeVault } from "./adapters/native"
export { gatewayVault } from "./adapters/gateway"
export type { VaultSource } from "./application/ports"
