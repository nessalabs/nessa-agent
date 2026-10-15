// Who may write the deletion inventory. `kind` is who is asking: the
// gateway recording a fence it already committed, a person, the gateway's
// own surface, or a device, peer, environment peer, or worker. `context.cause`
// is why: "committed-fence" after a deletion, unshare, or revocation, or
// "operator" when that principal asks for a capture. Both are facts. The
// permits and forbids below are the decision.

// The gateway records a fence that a command already committed.
@id("inventory.record.gateway")
permit (
    principal is NessaBackup::Principal,
    action == NessaBackup::Action::"deletion.inventory.record",
    resource is NessaBackup::Inventory
)
when {
    principal.kind == "gateway" &&
    context.cause == "committed-fence"
};

// The owner asks for a capture.
@id("inventory.capture.person")
permit (
    principal is NessaBackup::Principal,
    action == NessaBackup::Action::"deletion.inventory.capture",
    resource is NessaBackup::Inventory
)
when {
    principal.kind == "person" &&
    context.cause == "operator"
};

// The gateway's own surface asks for a capture.
@id("inventory.capture.surface")
permit (
    principal is NessaBackup::Principal,
    action == NessaBackup::Action::"deletion.inventory.capture",
    resource is NessaBackup::Inventory
)
when {
    principal.kind == "surface" &&
    context.cause == "operator"
};

@id("inventory.refuse.device")
forbid (
    principal is NessaBackup::Principal,
    action,
    resource is NessaBackup::Inventory
)
when { principal.kind == "device" };

@id("inventory.refuse.peer")
forbid (
    principal is NessaBackup::Principal,
    action,
    resource is NessaBackup::Inventory
)
when { principal.kind == "peer" };

@id("inventory.refuse.environment-peer")
forbid (
    principal is NessaBackup::Principal,
    action,
    resource is NessaBackup::Inventory
)
when { principal.kind == "environment-peer" };

@id("inventory.refuse.worker")
forbid (
    principal is NessaBackup::Principal,
    action,
    resource is NessaBackup::Inventory
)
when { principal.kind == "worker" };
