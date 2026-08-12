//! What Arin needs from the operating system, in terms every platform can answer in.
//!
//! Arin asks for one capability, reading the screen, on systems that each model consent
//! differently: TCC on macOS, the xdg desktop portal on Wayland, nothing at all on X11.
//! What they share is the set of answers a daemon has to act on, and that set lives here.
//! The asking itself is platform work and stays in the platform crate.
//!
//! # Why a granted permission is not the same as a working one
//!
//! Every one of these systems can report a grant that does not produce a frame. macOS
//! serves nothing to a process that was already running when the switch was flipped. A
//! portal session ends when the user revokes it. So [`Access`] keeps what the system
//! claims separate from what was proven, because those two disagreeing is the one state a
//! user cannot debug from the outside: everything looks correct and nothing works.
//!
//! # Implementing this on another platform
//!
//! Three rules, each of which cost something to learn on macOS and none of which is macOS
//! specific. A port that keeps them will not repeat the bug that produced this module.
//!
//! **Prove it, do not ask.** Report [`Access::Working`] only after a frame has actually
//! arrived. Every one of these systems has a state where it answers yes and then serves
//! nothing, and a backend that trusts the answer turns that into a silent failure with no
//! symptom to search for. The proving capture should be tiny: the question is whether a
//! frame arrives at all, not what is in it.
//!
//! **Separate "not granted" from "nothing to grant it to".** [`Access::Unidentified`] is
//! the variant nobody expects to need and everybody eventually needs, because these systems
//! remember a grant against an application identity rather than a name. On macOS that is a
//! code signature. On Wayland it is the app id a portal keys a restore token to. A build
//! with no stable identity reports exactly what an ungranted build reports, and only one of
//! them is fixed by the user doing anything at all, so collapsing the two sends people to
//! settings over and over for a problem that is not there.
//!
//! **Never take the screen to ask for the screen.** Whatever the platform's equivalent of
//! opening a settings pane is, a daemon started at login must not do it unprompted. That is
//! the whole of the bug fixed on 2026-08-11: an agent that started at login, found the
//! permission missing, and opened system settings every single time. Ask through the
//! mechanism the platform sanctions, then watch, and let a menu affordance be the way in.
//!
//! Where the variants land elsewhere, as a starting point rather than a specification. X11
//! is [`Access::NotRequired`], since any client can read the root window and there is no
//! switch to send anyone to. Wayland goes through the xdg desktop portal, where a live
//! session is [`Access::Working`] and no session or a revoked one is [`Access::Missing`].
//! [`Access::NeedsRestart`] is the one that may not map: it exists because macOS binds the
//! capture stream at process start, and a platform that re-negotiates per capture has no
//! equivalent and should never return it.

/// Whether Arin can read the screen, and why not when it cannot.
///
/// Ordered by how much a user has to do about it, which is also the order the variants are
/// worth checking in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Granted, and proven by taking a frame.
    Working,
    /// This platform needs no grant, so there is nothing to ask for.
    ///
    /// X11, where any client can read the root window. Distinct from [`Access::Working`]
    /// because there is no switch to send anyone to when something else goes wrong.
    NotRequired,
    /// The system reports the grant and capture still fails.
    ///
    /// On macOS this is what a grant made while the daemon was already running looks like.
    /// Restarting fixes it, and nothing else will.
    NeedsRestart,
    /// Not granted.
    Missing,
    /// There is no stable identity for a grant to attach to.
    ///
    /// A permission is remembered against the identity of the program holding it, so a
    /// program the system cannot identify can be granted the permission and lose it again
    /// on the next launch. To the user that reads as a grant that will not stick, or as a
    /// switch that is already on while nothing works.
    ///
    /// Worth its own variant because it is the only one here that no amount of clicking in
    /// system settings will clear, and because a user has no way to guess it.
    Unidentified,
}

impl Access {
    /// Whether capture can be expected to work.
    pub fn usable(self) -> bool {
        matches!(self, Self::Working | Self::NotRequired)
    }

    /// Whether the user has something to do before capture will work.
    ///
    /// [`Access::NeedsRestart`] counts. It is the state most likely to be mistaken for a
    /// working one, since the system itself reports the permission as granted.
    pub fn needs_the_user(self) -> bool {
        !self.usable()
    }
}

/// How Arin asks one platform about the screen recording permission.
///
/// The fifth platform seam, and the only one whose answers a user acts on directly, which
/// is why [`Permissions::explain`] is part of it rather than something callers assemble.
/// A state without the sentence that says what to do about it has sent people looking in
/// the wrong place before.
///
/// Implemented by `arin-mac` today. Linux and Windows implement it when their cycles come,
/// and the point of the trait is that the daemon and the command line do not change when
/// they do.
pub trait Permissions: Send + Sync + 'static {
    /// Whether capture works, proven rather than trusted.
    ///
    /// Expected to be expensive, since proving it means capturing. Callers that only need
    /// the system's own answer want [`Permissions::granted`].
    fn access(&self) -> Access;

    /// Whether the system reports the permission. Never prompts, never captures.
    ///
    /// Cheap enough for a menu that is opening. The gap between this and
    /// [`Permissions::access`] is the whole reason both exist.
    fn granted(&self) -> bool;

    /// Put the user in front of the switch. Reports whether that worked.
    fn open_settings(&self) -> bool;

    /// One line saying what the state means and what to do about it.
    fn explain(&self, access: Access) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_grant_or_no_grant_needed_counts_as_usable() {
        assert!(Access::Working.usable());
        assert!(Access::NotRequired.usable());

        // The trap. macOS reports the permission as granted here, so anything that trusted
        // the system's answer rather than a frame would call this usable and be wrong.
        assert!(!Access::NeedsRestart.usable());
        assert!(!Access::Missing.usable());
        assert!(!Access::Unidentified.usable());
    }

    #[test]
    fn every_unusable_state_is_one_the_user_has_to_act_on() {
        for access in [Access::NeedsRestart, Access::Missing, Access::Unidentified] {
            assert!(access.needs_the_user(), "{access:?}");
        }
        for access in [Access::Working, Access::NotRequired] {
            assert!(!access.needs_the_user(), "{access:?}");
        }
    }
}
