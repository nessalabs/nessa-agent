// Which machines may be handed the owner's saved logins when an agent
// starts there (issue #1260). Facts only: what kind of machine it is, and
// whether its owner switched it on. Everything else is denied.

// This gateway logs in as an SSH host's account, or a Boat workspace's: it
// can already set anything a harness there runs with.
@id("login.own-account")
permit (principal is NessaLogins::Machine, action == NessaLogins::Action::"login.receive", resource is NessaLogins::Login)
when { principal.reach == "ssh" || principal.reach == "boat" };

// A paired machine dials in under its own owner's account: only once its
// switch in Devices is on.
@id("login.paired-switched-on")
permit (principal is NessaLogins::Machine, action == NessaLogins::Action::"login.receive", resource is NessaLogins::Login)
when { principal.reach == "peer" && principal.switchedOn };

// This gateway's own machine reads its own logins; a login is never handed
// to it over a lease.
@id("login.not-here")
forbid (principal is NessaLogins::Machine, action, resource is NessaLogins::Login)
when { principal.reach == "here" };
