// Where a turn may run commands, and which programs (ADR 803 step 4). A host
// no policy permits is refused, and so is every program on it.
//
// `resource.gateway` is what this gateway is configured with
// (`environmentTools`): the hosts commands are granted on, whether it has an
// allow-list, the programs on it, and the programs denied. `context.program`
// is the program as the agent named it, bare or a path; `context.programName`
// is its file name, the part after its last `/`, so `/usr/bin/rm` is `rm`.

// The owner's turns use every host the gateway grants commands on.
@id("command.owner_host")
permit (
  principal is Nessa::Owner,
  action in [Nessa::Action::"command.host", Nessa::Action::"command.run"],
  resource is Nessa::Host
)
when { resource.gateway.commandHosts.contains(resource.name) };

// A grant's turns use a host only when the gateway grants commands on it and
// the grant names it too. A grant that names no hosts uses none: a Drive turn
// does not inherit the owner's hosts.
@id("command.grant_host")
permit (
  principal is Nessa::Grant,
  action in [Nessa::Action::"command.host", Nessa::Action::"command.run"],
  resource is Nessa::Host
)
when {
  resource.gateway.commandHosts.contains(resource.name) &&
  principal has commandHosts &&
  principal.commandHosts.contains(resource.name)
};

// An agent-authored turn does not inherit command authority from the person
// whose conversation received it. Keep the denial in Cedar so both listing
// and running a command receive the same policy decision and reason.
@id("command.agent_host")
forbid (
  principal is Nessa::Agent,
  action in [Nessa::Action::"command.host", Nessa::Action::"command.run"],
  resource is Nessa::Host
);

// With an allow-list, only a program named bare, by a name on it. The host
// finds a bare name only in absolute search directories outside its
// workspace, so a file the agent wrote under an allowed name is not run, and
// `/tmp/x/cargo` is not `cargo`.
@id("command.unlisted_program")
forbid (
  principal,
  action == Nessa::Action::"command.run",
  resource is Nessa::Host
)
when {
  resource.gateway.allowListed &&
  !(context.program == context.programName &&
    resource.gateway.allowPrograms.contains(context.programName))
};

// A denied program, by its file name wherever it is: a denial wins over an
// allowance.
@id("command.denied_program")
forbid (
  principal,
  action == Nessa::Action::"command.run",
  resource is Nessa::Host
)
when { resource.gateway.denyPrograms.contains(context.programName) };
