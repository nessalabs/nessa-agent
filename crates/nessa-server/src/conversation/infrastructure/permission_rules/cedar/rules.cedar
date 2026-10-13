// Creating, listing, and revoking a remembered permission rule is the owner's.
// A shared user is a Grant, and nothing here permits one. Settling a review
// is not in this file: each stored rule is its own policy, built from the row.

@id("rule.grant")
permit (
  principal is Nessa::Owner,
  action == Nessa::Action::"rule.grant",
  resource is Nessa::Tool
);

@id("rule.revoke")
permit (
  principal is Nessa::Owner,
  action == Nessa::Action::"rule.revoke",
  resource is Nessa::Tool
);

@id("rule.list")
permit (
  principal is Nessa::Owner,
  action == Nessa::Action::"rule.list",
  resource is Nessa::Tool
);
