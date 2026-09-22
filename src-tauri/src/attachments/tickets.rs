//! One-shot tickets: what the panel is given instead of a path it can hand
//! back.
//!
//! The command that reads a chosen file's bytes used to take a path. Any path.
//! The webview named one and the host opened it, with no correlation to
//! anything the picker had ever returned, following symbolic links on the way.
//! The 64 MiB bound limited how much came back, not what could be reached, so a
//! page that had been talked into asking for `~/.ssh/id_ed25519` was answered.
//!
//! A ticket closes that by inverting who holds the path. The picker mints an
//! unguessable token per chosen file, keeps the path beside it here in the
//! host, and hands the panel only the token. [`super::read_attachment_bytes`]
//! takes the token, and the only paths it can resolve are ones this host
//! chose to remember because a person picked them. `ChosenFile.path` is still
//! in the answer — the panel sends it to the gateway, which is the entire
//! feature — but it has stopped being what authorises a read.
//!
//! ```text
//!   picker ──▶ mint(path) ──▶ TicketBook ──┐
//!                                 │        └──▶ secret ──▶ panel
//!                                 │                          │
//!   read_attachment_bytes ◀───────┼────────── ticket ◀───────┘
//!                                 ▼
//!                        redeem ──▶ Path | AlreadyUsed | Expired | Unknown
//! ```
//!
//! **Unguessable.** Thirty-two bytes from the operating system's random source,
//! written as sixty-four lowercase hexadecimal digits — the same strength and
//! the same shape as the upload tickets in
//! `crates/nessa-server/src/attachments`, deliberately, because a second,
//! weaker way of being unguessable in the same product is a hole waiting to be
//! found. The book keeps the SHA-256 fingerprint rather than the secret, so the
//! secret exists on its way out to the panel and on its way back and nowhere
//! else; [`TicketSecret`] has no `Display` and a `Debug` that prints nothing,
//! so it cannot reach a log by being formatted.
//!
//! **Redeemable once.** Redeeming takes the path out of the entry and leaves
//! the entry behind. That is what makes "you have already read that" a
//! different answer from "I have never heard of that", which is a difference
//! worth a whole entry: the first is a bug in the panel or a replay, and the
//! second is a ticket that expired or was pushed out. A spent entry holds no
//! path, so the memory that mattered is released at the moment it is spent.
//!
//! **Bounded, and why these numbers.** [`MOST_TICKETS`] is 1024 — fifty times
//! the panel's own cap of twenty attachments to a draft, which is what makes it
//! comfortably past any selection a person assembles by hand, and about two
//! hundred kilobytes of paths, which is what makes it a ceiling nobody can
//! feel. The oldest give way rather than the newest being refused: a person who
//! has just picked a file must be able to attach it, and the tickets that would
//! be lost first are the ones nothing has redeemed in the longest time. The
//! bound alone is what caps memory, so it holds whether or not anything ever
//! expires.
//!
//! [`TICKET_LIFETIME`] is ten minutes. In practice the panel redeems within a
//! second of picking — it reads an image's bytes as soon as the choice comes
//! back — so ten minutes is not a working window but a margin: it covers a
//! machine that stalled, a person interrupted mid-upload, and a retry, and it
//! stops a ticket forgotten in a long-lived page from being a standing key to
//! a path for the rest of the session.
//!
//! Expiry is read at redemption rather than swept on a timer, and that is a
//! choice with a consequence worth naming: an expired ticket says so, honestly,
//! for as long as its entry survives the bound, and becomes indistinguishable
//! from an unknown one only once it has been pushed out. A sweep would have
//! collapsed that distinction immediately and bought nothing, because the bound
//! is already what caps memory.
//!
//! One thing the bound quietly cleans up after, said plainly. A pick that is
//! abandoned at its deadline (see [`super::files`]) can have minted a ticket
//! whose secret then never reaches the panel, because the answer it was part of
//! was thrown away. Such a ticket is stranded: nobody holds the only thing that
//! could redeem it, so it is unreachable rather than dangerous, and it leaves
//! on the same terms as any other — pushed out by [`MOST_TICKETS`], or unable
//! to be redeemed once its lifetime has passed.
//!
//! **What is not verified here.** [`MintedTickets`] is the adapter and it is
//! thin on purpose: it reads the clock, it asks the operating system for
//! randomness, it takes a lock. Nothing tests that `getrandom` is random or
//! that `Instant::now` advances. [`TicketBook`] is where every rule lives and
//! it takes the clock as a parameter, so single use, expiry, eviction and the
//! order they are decided in are all written down below without a clock, a
//! random source, or a filesystem.

use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// How many tickets the host keeps at once. See the module header for why
/// 1024 and why the oldest give way.
pub const MOST_TICKETS: usize = 1024;

/// How long a ticket stays redeemable. See the module header for why ten
/// minutes is a margin rather than a working window.
pub const TICKET_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// The secret half of a ticket: 32 random bytes, written as 64 lowercase
/// hexadecimal digits.
///
/// It exists only on its way to the panel and on its way back; the book keeps
/// its fingerprint. It has no `Display`, and `Debug` prints nothing, so it
/// cannot reach a log by being formatted.
#[derive(Clone, PartialEq, Eq)]
pub struct TicketSecret([u8; 32]);

impl TicketSecret {
    /// A secret from 32 bytes of randomness the caller has already obtained.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Exactly 64 lowercase hexadecimal digits, or nothing. Anything else is
    /// not a ticket this host ever minted, and is not worth looking up.
    pub fn parse(value: &str) -> Option<Self> {
        if value.len() != 64 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (byte, digits) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
            *byte = (nibble(digits[0])? << 4) | nibble(digits[1])?;
        }
        Some(Self(bytes))
    }

    /// The text to hand to the panel, once.
    pub fn expose(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// What the book keeps instead of the secret.
    fn fingerprint(&self) -> [u8; 32] {
        Sha256::digest(self.0).into()
    }
}

impl fmt::Debug for TicketSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TicketSecret(..)")
    }
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

/// What presenting a ticket found.
///
/// Four outcomes because the panel acts on all four differently, and because
/// collapsing any two of them would put the wrong sentence on screen: only the
/// first reads a file, and the other three are refusals with different ways
/// out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redeemed {
    /// The ticket was outstanding and in time. The path it named is now spent
    /// and this is the only time it will be answered.
    Path(PathBuf),
    /// The ticket was minted and has already bought its read.
    AlreadyUsed,
    /// The ticket was minted, was never spent, and has run out of time.
    Expired,
    /// Never minted, or minted long enough ago to have been pushed out by
    /// [`MOST_TICKETS`].
    Unknown,
}

/// The host has no way to mint a ticket, in its own words.
///
/// Its own failure because it is not the file's fault and not the person's:
/// the operating system's random source is what did not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoTicket {
    /// What happened, for the diagnostics.
    pub detail: String,
}

/// Where the panel's one-shot tickets are minted and spent.
///
/// A port because both halves reach outside the process — randomness for the
/// mint, the clock for the expiry — and because a substitute has to be able to
/// stage every refusal the panel can be given, including the two no real desk
/// can be made to produce on demand: a ticket presented twice, and a ticket
/// presented too late.
pub trait AttachmentTickets: Send + Sync {
    /// A fresh ticket for `path`, or why there is none.
    ///
    /// The path is remembered here and the caller gets only the secret. Minting
    /// may push the oldest outstanding ticket out; see [`MOST_TICKETS`].
    fn mint(&self, path: &Path) -> Result<String, NoTicket>;

    /// Spends a ticket, answering with the path it named or with why it named
    /// nothing. A ticket is spent by this call whether or not the read that
    /// follows succeeds — a retry is a new choice, not a second use.
    fn redeem(&self, ticket: &str) -> Redeemed;
}

/// One minted ticket, as the book keeps it.
///
/// `path` is an `Option` because a stub that can no longer be redeemed — spent
/// or expired — lets go of it: the entry stays behind to say which of the two
/// happened, and holds nothing worth holding while it does.
///
/// `spent` is a fact of its own rather than "the path is gone", and the
/// difference is the whole reason it is here. Both a spent ticket and an
/// expired one have let go of their path, and answering both with "already
/// used" would tell somebody to stop doing something they never did.
#[derive(Debug)]
struct Stub {
    fingerprint: [u8; 32],
    path: Option<PathBuf>,
    minted: Instant,
    spent: bool,
}

/// Every outstanding ticket, and all the rules about them.
///
/// Pure: the clock arrives as a parameter, so single use, expiry, eviction and
/// the order they are decided in are written down without a clock to wait for.
/// The bound and the lifetime arrive at construction for the same reason — a
/// test says ten milliseconds and the app says ten minutes, and neither knows
/// the difference.
#[derive(Debug)]
pub struct TicketBook {
    most: usize,
    lifetime: Duration,
    /// Oldest first, which is the order eviction needs and the order minting
    /// produces.
    minted: VecDeque<Stub>,
}

impl TicketBook {
    /// A book holding at most `most` tickets, each redeemable for `lifetime`.
    pub fn new(most: usize, lifetime: Duration) -> Self {
        Self {
            most,
            lifetime,
            minted: VecDeque::new(),
        }
    }

    /// How many tickets are on the books, spent or not. The number the bound
    /// is a bound on.
    ///
    /// Test-only because the app has nothing to do with the answer: the bound
    /// enforces itself in [`TicketBook::mint`], and the one caller that needs
    /// to see it is the test that holds the bound to its word. Compiled into
    /// the shipped binary it would be dead code, which `-D warnings` calls an
    /// error, and an `allow` would be a lie about why it is here.
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.minted.len()
    }

    /// Records a ticket for `path`, pushing out the oldest if the book is full.
    ///
    /// Never refuses. A person who has just picked a file has to be able to
    /// attach it, so a full book gives up its oldest ticket rather than its
    /// newest choice — and the ticket given up is the one nothing has redeemed
    /// in the longest time.
    pub fn mint(&mut self, secret: &TicketSecret, path: PathBuf, now: Instant) {
        self.minted.push_back(Stub {
            fingerprint: secret.fingerprint(),
            path: Some(path),
            minted: now,
            spent: false,
        });
        while self.minted.len() > self.most {
            self.minted.pop_front();
        }
    }

    /// Spends a ticket.
    ///
    /// The order is the rule. A ticket the book has never heard of is unknown
    /// before anything else is considered. A ticket that has been spent says so
    /// even if its time has since run out, because what the caller did is the
    /// more useful fact than what the clock did afterwards. Only a ticket that
    /// is both unspent and in time answers with a path — and answers with it
    /// once, because taking the path is what spends the ticket.
    ///
    /// Both refusals let go of the path they were keeping. Neither a spent
    /// ticket nor an expired one can ever name a file again, so holding the
    /// path would be holding a path nothing can reach.
    pub fn redeem(&mut self, secret: &TicketSecret, now: Instant) -> Redeemed {
        let fingerprint = secret.fingerprint();
        let lifetime = self.lifetime;
        let Some(stub) = self
            .minted
            .iter_mut()
            .find(|stub| stub.fingerprint == fingerprint)
        else {
            return Redeemed::Unknown;
        };

        if stub.spent {
            return Redeemed::AlreadyUsed;
        }
        if now.duration_since(stub.minted) >= lifetime {
            stub.path = None;
            return Redeemed::Expired;
        }
        stub.spent = true;
        match stub.path.take() {
            Some(path) => Redeemed::Path(path),
            // Unspent, in time, and holding nothing: there is no way to build
            // such a stub, and answering "unknown" is the one refusal that
            // claims nothing about what happened.
            None => Redeemed::Unknown,
        }
    }
}

/// The real desk: the operating system's randomness, this machine's clock, and
/// one book behind a lock.
struct MintedTickets(Mutex<TicketBook>);

impl MintedTickets {
    /// The book, whatever a previous panic left behind.
    ///
    /// A poisoned lock is recovered from rather than propagated: the book is a
    /// list of stubs and cannot be left half-written by an unwind, and a host
    /// that refused every attachment for the rest of the session because one
    /// thread panicked once would be a worse failure than the one it is
    /// reporting.
    fn book(&self) -> MutexGuard<'_, TicketBook> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl AttachmentTickets for MintedTickets {
    fn mint(&self, path: &Path) -> Result<String, NoTicket> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|error| NoTicket {
            detail: format!("the operating system's random source did not answer: {error}"),
        })?;
        let secret = TicketSecret::from_bytes(bytes);
        self.book()
            .mint(&secret, path.to_path_buf(), Instant::now());
        Ok(secret.expose())
    }

    fn redeem(&self, ticket: &str) -> Redeemed {
        // Anything that is not 64 hexadecimal digits was never minted here, so
        // it is unknown without the book being consulted or a lock being taken.
        let Some(secret) = TicketSecret::parse(ticket) else {
            return Redeemed::Unknown;
        };
        self.book().redeem(&secret, Instant::now())
    }
}

/// Where chosen files' tickets are kept. Called from composition.
pub fn attachment_tickets() -> Arc<dyn AttachmentTickets> {
    Arc::new(MintedTickets(Mutex::new(TicketBook::new(
        MOST_TICKETS,
        TICKET_LIFETIME,
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinct secret per number, without a random source: the book cares
    /// only that fingerprints differ.
    fn secret(nth: u8) -> TicketSecret {
        let mut bytes = [0_u8; 32];
        bytes[0] = nth;
        TicketSecret::from_bytes(bytes)
    }

    fn path(name: &str) -> PathBuf {
        ["/Users/dev/Pictures", name].iter().collect()
    }

    /// A book with room and a long life, for the cases that are about neither.
    fn book() -> TicketBook {
        TicketBook::new(8, Duration::from_secs(600))
    }

    #[test]
    fn a_minted_ticket_answers_with_the_path_it_was_minted_for() {
        let mut book = book();
        let now = Instant::now();

        book.mint(&secret(1), path("holiday.heic"), now);

        assert_eq!(
            book.redeem(&secret(1), now),
            Redeemed::Path(path("holiday.heic"))
        );
    }

    /// The whole point of the scheme: one ticket buys one read, and the second
    /// attempt is told apart from a ticket that never existed.
    #[test]
    fn a_ticket_is_redeemable_once_and_says_so_afterwards() {
        let mut book = book();
        let now = Instant::now();
        book.mint(&secret(1), path("holiday.heic"), now);

        assert!(matches!(book.redeem(&secret(1), now), Redeemed::Path(_)));

        assert_eq!(book.redeem(&secret(1), now), Redeemed::AlreadyUsed);
        assert_eq!(book.redeem(&secret(1), now), Redeemed::AlreadyUsed);
    }

    /// One ticket is one file. Minting two and spending one must leave the
    /// other alone — a book that answered by position rather than by
    /// fingerprint would pass every test above and fail this one.
    #[test]
    fn one_tickets_use_does_not_spend_another() {
        let mut book = book();
        let now = Instant::now();
        book.mint(&secret(1), path("first.png"), now);
        book.mint(&secret(2), path("second.png"), now);

        assert_eq!(
            book.redeem(&secret(1), now),
            Redeemed::Path(path("first.png"))
        );

        assert_eq!(
            book.redeem(&secret(2), now),
            Redeemed::Path(path("second.png"))
        );
    }

    /// A secret nobody minted names nothing, whatever else is in the book.
    #[test]
    fn a_secret_that_was_never_minted_is_unknown() {
        let mut book = book();
        let now = Instant::now();
        book.mint(&secret(1), path("holiday.heic"), now);

        assert_eq!(book.redeem(&secret(9), now), Redeemed::Unknown);
    }

    /// The lifetime, at its two edges. A ticket presented at the last instant
    /// still works; one presented at the first instant past does not.
    #[test]
    fn a_ticket_stops_working_once_its_lifetime_has_passed() {
        let lifetime = Duration::from_secs(600);
        let minted = Instant::now();

        let mut in_time = TicketBook::new(8, lifetime);
        in_time.mint(&secret(1), path("holiday.heic"), minted);
        assert!(matches!(
            in_time.redeem(&secret(1), minted + lifetime - Duration::from_millis(1)),
            Redeemed::Path(_)
        ));

        let mut too_late = TicketBook::new(8, lifetime);
        too_late.mint(&secret(1), path("holiday.heic"), minted);
        assert_eq!(
            too_late.redeem(&secret(1), minted + lifetime),
            Redeemed::Expired
        );
    }

    /// And an expired ticket is expired for good, and still says *expired*: a
    /// clock that went backwards must not find the path again, and a person who
    /// never read the file must not be told they already did.
    #[test]
    fn an_expired_ticket_stays_expired_and_never_finds_its_path_again() {
        let lifetime = Duration::from_secs(600);
        let minted = Instant::now();
        let mut book = TicketBook::new(8, lifetime);
        book.mint(&secret(1), path("holiday.heic"), minted);

        assert_eq!(
            book.redeem(&secret(1), minted + lifetime),
            Redeemed::Expired
        );

        assert_eq!(
            book.redeem(&secret(1), minted + lifetime),
            Redeemed::Expired
        );
        assert_eq!(book.redeem(&secret(1), minted), Redeemed::Unknown);
    }

    /// What the caller did beats what the clock did afterwards: a ticket that
    /// was spent and then ran out of time reports the use, because that is the
    /// fact somebody can act on.
    #[test]
    fn a_spent_ticket_reports_its_use_rather_than_its_expiry() {
        let lifetime = Duration::from_secs(600);
        let minted = Instant::now();
        let mut book = TicketBook::new(8, lifetime);
        book.mint(&secret(1), path("holiday.heic"), minted);
        assert!(matches!(book.redeem(&secret(1), minted), Redeemed::Path(_)));

        assert_eq!(
            book.redeem(&secret(1), minted + lifetime * 2),
            Redeemed::AlreadyUsed
        );
    }

    /// The bound, doing the one job it has: a panel that picks for an hour
    /// without reading anything does not grow the host's memory.
    #[test]
    fn the_book_never_holds_more_than_its_bound() {
        let mut book = TicketBook::new(4, Duration::from_secs(600));
        let now = Instant::now();

        for nth in 0..64 {
            book.mint(&secret(nth), path("unread.bin"), now);
        }

        assert_eq!(book.held(), 4);
    }

    /// And it gives up the oldest rather than refusing the newest: the file
    /// somebody just picked is the one that has to work.
    #[test]
    fn a_full_book_gives_up_its_oldest_ticket_not_its_newest() {
        let mut book = TicketBook::new(2, Duration::from_secs(600));
        let now = Instant::now();
        book.mint(&secret(1), path("oldest.png"), now);
        book.mint(&secret(2), path("middle.png"), now);
        book.mint(&secret(3), path("newest.png"), now);

        assert_eq!(book.redeem(&secret(1), now), Redeemed::Unknown);
        assert_eq!(
            book.redeem(&secret(3), now),
            Redeemed::Path(path("newest.png"))
        );
    }

    /// The secret's shape is the contract with the panel, which carries it as
    /// text and hands it straight back.
    #[test]
    fn a_secret_is_sixty_four_lowercase_hexadecimal_digits_and_survives_the_round_trip() {
        let secret = TicketSecret::from_bytes([0xab; 32]);

        let exposed = secret.expose();

        assert_eq!(exposed.len(), 64);
        assert!(
            exposed
                .chars()
                .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase()),
            "{exposed}"
        );
        assert_eq!(TicketSecret::parse(&exposed), Some(secret));
    }

    /// Anything else the webview might hand in is not a ticket, and is refused
    /// without the book being consulted.
    #[test]
    fn nothing_but_sixty_four_hexadecimal_digits_parses_as_a_ticket() {
        for offered in [
            "",
            "abc",
            &"a".repeat(63),
            &"a".repeat(65),
            &"A".repeat(64),
            &"g".repeat(64),
            "/Users/dev/.ssh/id_ed25519",
        ] {
            assert_eq!(TicketSecret::parse(offered), None, "{offered:?}");
        }
    }

    /// The secret cannot reach a log by being formatted, which is the one way
    /// a value that is only ever passed through tends to escape.
    #[test]
    fn a_secret_prints_nothing_about_itself() {
        let printed = format!("{:?}", TicketSecret::from_bytes([0xab; 32]));

        assert_eq!(printed, "TicketSecret(..)");
        assert!(!printed.contains("ab"), "{printed}");
    }
}
