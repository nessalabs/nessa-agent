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

// A person or the gateway surface may stage, accept, or fetch a deferred
// conversation when they are the operator. A device, peer, environment
// peer, or worker may not.

@id("restore.stage.person")
permit (
    principal is NessaBackup::Principal,
    action == NessaBackup::Action::"backup.restore.stage",
    resource is NessaBackup::Restore
)
when {
    principal.kind == "person" &&
    context.cause == "operator"
};

@id("restore.stage.surface")
permit (
    principal is NessaBackup::Principal,
    action == NessaBackup::Action::"backup.restore.stage",
    resource is NessaBackup::Restore
)
when {
    principal.kind == "surface" &&
    context.cause == "operator"
};

@id("restore.accept.person")
permit (
    principal is NessaBackup::Principal,
    action == NessaBackup::Action::"backup.restore.accept",
    resource is NessaBackup::Restore
)
when {
    principal.kind == "person" &&
    context.cause == "operator"
};

@id("restore.accept.surface")
permit (
    principal is NessaBackup::Principal,
    action == NessaBackup::Action::"backup.restore.accept",
    resource is NessaBackup::Restore
)
when {
    principal.kind == "surface" &&
    context.cause == "operator"
};

@id("restore.fetch.person")
permit (
    principal is NessaBackup::Principal,
    action == NessaBackup::Action::"backup.deferred.fetch",
    resource is NessaBackup::Restore
)
when {
    principal.kind == "person" &&
    context.cause == "operator"
};

@id("restore.fetch.surface")
permit (
    principal is NessaBackup::Principal,
    action == NessaBackup::Action::"backup.deferred.fetch",
    resource is NessaBackup::Restore
)
when {
    principal.kind == "surface" &&
    context.cause == "operator"
};

@id("restore.refuse.device")
forbid (
    principal is NessaBackup::Principal,
    action,
    resource is NessaBackup::Restore
)
when { principal.kind == "device" };

@id("restore.refuse.peer")
forbid (
    principal is NessaBackup::Principal,
    action,
    resource is NessaBackup::Restore
)
when { principal.kind == "peer" };

@id("restore.refuse.environment-peer")
forbid (
    principal is NessaBackup::Principal,
    action,
    resource is NessaBackup::Restore
)
when { principal.kind == "environment-peer" };

@id("restore.refuse.worker")
forbid (
    principal is NessaBackup::Principal,
    action,
    resource is NessaBackup::Restore
)
when { principal.kind == "worker" };
