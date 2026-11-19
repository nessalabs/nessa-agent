// A directory grant's capabilities, the path walk, and the sandbox the
// environment says it enforces. Command matchers are built per grant.

@id("directory.read")
permit (
  principal is NessaDirectory::Grant,
  action == NessaDirectory::Action::"directory.read",
  resource is NessaDirectory::Directory
)
when {
  principal.capabilities.contains("read") &&
  context.leave == "inside" &&
  !context.marker
};

@id("directory.write")
permit (
  principal is NessaDirectory::Grant,
  action == NessaDirectory::Action::"directory.write",
  resource is NessaDirectory::Directory
)
when {
  principal.capabilities.contains("write") &&
  context.leave == "inside" &&
  !context.marker
};

@id("directory.exec")
permit (
  principal is NessaDirectory::Grant,
  action == NessaDirectory::Action::"directory.exec",
  resource is NessaDirectory::Directory
)
when {
  principal.capabilities.contains("exec") &&
  context.leave == "inside" &&
  !context.marker &&
  context.profile != "none" &&
  context.hidesMarker &&
  context.rootLeave == "inside"
};

@id("directory.path")
forbid (
  principal,
  action in [
    NessaDirectory::Action::"directory.read",
    NessaDirectory::Action::"directory.write",
    NessaDirectory::Action::"directory.exec"
  ],
  resource
)
when { context.leave != "inside" };

@id("directory.marker")
forbid (
  principal,
  action in [
    NessaDirectory::Action::"directory.read",
    NessaDirectory::Action::"directory.write",
    NessaDirectory::Action::"directory.exec"
  ],
  resource
)
when { context.marker };

@id("directory.exec_none")
forbid (
  principal,
  action == NessaDirectory::Action::"directory.exec",
  resource
)
when { context.profile == "none" };

@id("directory.exec_marker")
forbid (
  principal,
  action == NessaDirectory::Action::"directory.exec",
  resource
)
when { !context.hidesMarker };

@id("directory.exec_root")
forbid (
  principal,
  action == NessaDirectory::Action::"directory.exec",
  resource
)
when { context.rootLeave != "inside" };
