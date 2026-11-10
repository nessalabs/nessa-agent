/**
 * Devices: the hub's machines, as the side rail's Devices page shows them.
 * `ui/add-gateway.tsx` creates PeerRead invitations;
 * `adapters/gateway-pairing.ts` owns the separate explicit Human session.
 * `ui/gateway-invitation.ts` keeps selected page facts together;
 * `model/gateway-pairing.ts` formats the quoted enrollment command.
 * Phone and worker consent stay with their existing adapters.
 * Composed in `main.tsx` (its gateway) and `desktop-window.tsx` (its page).
 */
export {
  devicesGateway,
  type DevicesClient,
  type DevicesGateway,
} from "./adapters/devices-gateway"
export { DevicesProvider, DevicesView, useDevicesGateway } from "./ui/devices-view"
export { PoolOrder } from "./ui/pool-order"
