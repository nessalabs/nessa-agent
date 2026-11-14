/// What asking this machine one question about an agent produced.
///
/// Three answers rather than two, because "no" and "I could not find out" are
/// different facts and lead to different advice. A locked keychain, a missing
/// home directory, and a tool that will not run are all "I could not find out";
/// treating them as "no" would tell the person something the machine never
/// said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostAnswer {
    /// The machine answered yes.
    Yes,
    /// The machine answered no.
    No,
    /// The machine could not be asked, or would not answer.
    Undetermined,
}
