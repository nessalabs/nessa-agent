import contract from "../../../protocol/defaults/vault-contract.json"
import type { VaultFailure } from "../application/ports"
export const vaultFailureCopy: Record<VaultFailure, string> = {
  vault_forbidden:
    "This connection cannot manage your vault. Use the owning app session.",
  vault_unavailable: "Secure storage is unavailable. Try again.",
  vault_locked: "Your vault is locked.",
  vault_passphrase_policy:
    "This passphrase is too short or too predictable. Choose longer, unique words.",
  vault_wrong_passphrase:
    "That passphrase didn't unlock the vault. Check it and try again.",
  vault_busy: "Secure storage is busy. Try again shortly.",
  vault_cooldown: `Too many attempts. Wait ${contract.cooldownSeconds} seconds, then try again.`,
  vault_session_closed: "The app connection ended. Try again.",
  vault_session_owned:
    "Another app session has the vault unlocked. Close it and try again.",
  vault_already_configured: "A vault already exists. Unlock it to continue.",
  vault_capacity: "Secure storage is full.",
  vault_audit_unavailable: "Nessa couldn't record the change. Try again.",
  vault_migration_unavailable:
    "Nessa couldn't import existing protected data. Unlock the old OS credential store once, then try again. Your existing data is preserved.",
  vault_change_unchanged:
    "The passphrase was not changed. Your vault is locked; unlock with the old passphrase.",
  vault_change_unchanged_audit_unavailable:
    "The passphrase was not changed, and Nessa could not record the outcome. Your vault is locked; unlock with the old passphrase.",
  vault_change_not_confirmed:
    "The change was not confirmed. Unlock with the new passphrase first; if it fails, try the old passphrase. Nessa will not retry the change automatically.",
  vault_change_not_confirmed_audit_unavailable:
    "The change and its audit record were not confirmed. Unlock with the new passphrase first; if it fails, try the old passphrase. Nessa will not retry the change automatically.",
  vault_changed_audit_unavailable:
    "Your passphrase changed, but Nessa could not record the outcome. Your vault is locked; unlock with the new passphrase.",
}
