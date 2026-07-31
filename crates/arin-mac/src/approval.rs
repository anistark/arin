//! Asking the user whether a client may make Arin look at the screen.
//!
//! The macOS half of the capability split in `arin_core::consent`. Core decides when to ask
//! and what the answer means. This decides what asking looks like, which on a Mac is an
//! `NSAlert`.
//!
//! # Why an alert and not the overlay
//!
//! The overlay is 100 percent click through and has no buttons in it, which is a rule
//! rather than an implementation detail. A consent prompt needs to be clicked, so it cannot
//! live there. It is also the one thing in Arin that must interrupt: a mark nobody notices
//! is a wasted mark, and a permission prompt nobody notices is a permission nobody granted.
//!
//! # Why the alert says which resolver would run
//!
//! Granting a model on this machine a look at the screen and granting a hosted one a copy
//! of it are different decisions, and the daemon is the only thing that knows which is
//! being asked for. A prompt that said "allow grounding" would be asking about the wrong
//! thing.

use arin_core::consent::{Decision, Request};
use arin_core::{Approver, traits};
use dispatch2::DispatchQueue;
use futures::future::BoxFuture;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle, NSApplication,
};
use objc2_foundation::NSString;
use std::time::Duration;

/// How long "allow for a while" lasts.
///
/// An hour. Long enough that a working session does not keep interrupting, which is what
/// decides whether anybody leaves this switched on, and short enough that a grant given
/// once does not quietly cover tomorrow. It is not persisted, so a restart ends it anyway.
pub const WINDOW: Duration = Duration::from_secs(3600);

/// Puts grounding requests in front of the user as a modal alert.
pub struct AlertApprover;

impl Approver for AlertApprover {
    fn ask<'a>(&'a self, request: &'a Request) -> BoxFuture<'a, Decision> {
        // Built here, on the daemon's thread, so the block dispatched below owns plain
        // strings rather than a borrow of something living on another thread.
        let title = format!("Let {} see your screen?", request.client_name);
        let body = describe(request);

        let (tx, rx) = futures::channel::oneshot::channel();
        DispatchQueue::main().exec_async(move || {
            let decision =
                MainThreadMarker::new().map_or(Decision::Deny, |mtm| present(mtm, &title, &body));
            let _ = tx.send(decision);
        });

        // A prompt that cannot be delivered is a no. The channel closes if the block never
        // runs or panics, and defaulting to yes there would mean the gate opens exactly
        // when something has gone wrong.
        Box::pin(async move { rx.await.unwrap_or(Decision::Deny) })
    }
}

/// What the alert says under its title.
///
/// The query is quoted because it is the most informative part of the request: "the Submit
/// button" and "the row showing the account balance" are the same thing to the daemon and
/// very different things to read.
fn describe(request: &Request) -> String {
    let egress = if request.remote {
        format!(
            "\n\nA screenshot of your display will be SENT OFF THIS MACHINE to the \
             {} resolver.",
            request.resolver
        )
    } else {
        format!(
            "\n\nGrounding runs on this machine through the {} resolver. Nothing is sent \
             anywhere.",
            request.resolver
        )
    };

    format!(
        "{} asked Arin to find:\n\n    \u{201c}{}\u{201d}\n\nArin will take a screenshot to \
         work out where that is, and tell the client the coordinates. Arin holds Screen \
         Recording permission and this client does not, so answering yes lets it read your \
         screen through Arin.{egress}\n\nAllowing for an hour covers anything asked for in \
         that time, by any program running as you. You can take it back from the menu bar.",
        request.client_name, request.query
    )
}

/// Run the alert and read the button.
///
/// Main thread only, which is what the marker proves.
fn present(mtm: MainThreadMarker, title: &str, body: &str) -> Decision {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(body));
    // Critical rather than Warning: this is a permission grant and it should not look like
    // a notice somebody can dismiss without reading.
    alert.setAlertStyle(NSAlertStyle::Critical);

    // Order matters. The first button is the default, and the default here has to be the
    // narrow answer rather than the wide one, so leaning on return grants the least.
    alert.addButtonWithTitle(&NSString::from_str("Allow Once"));
    alert.addButtonWithTitle(&NSString::from_str("Allow for an Hour"));
    alert.addButtonWithTitle(&NSString::from_str("Deny"));

    // Arin is a menu bar app with no windows of its own, so without this the alert can come
    // up behind whatever the user is looking at, which is where an unanswered prompt looks
    // like a hung client.
    NSApplication::sharedApplication(mtm).activate();

    match alert.runModal() {
        r if r == NSAlertFirstButtonReturn => Decision::Once,
        r if r == NSAlertSecondButtonReturn => Decision::For(WINDOW),
        // Anything else, including the alert being dismissed some other way, is a no.
        _ => Decision::Deny,
    }
}

/// The trait object the daemon wires up.
pub fn approver() -> std::sync::Arc<dyn traits::Approver> {
    std::sync::Arc::new(AlertApprover)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(remote: bool) -> Request {
        Request {
            client_name: "claude-code".into(),
            query: "the row showing the account balance".into(),
            resolver: if remote { "claude" } else { "local" }.into(),
            remote,
        }
    }

    /// The prompt has to say what the answer costs, and the two answers cost different
    /// things. A prompt that read the same either way would be asking the wrong question.
    #[test]
    fn the_prompt_says_whether_the_screen_leaves_the_machine() {
        let remote = describe(&request(true));
        assert!(remote.contains("SENT OFF THIS MACHINE"), "got {remote}");
        assert!(remote.contains("claude"));

        let local = describe(&request(false));
        assert!(local.contains("Nothing is sent anywhere"), "got {local}");
        assert!(!local.contains("SENT OFF THIS MACHINE"));
    }

    /// The query is the informative part, and the reason the prompt is worth showing at
    /// all rather than a blanket setting.
    #[test]
    fn the_prompt_quotes_what_was_actually_asked_for() {
        let body = describe(&request(false));
        assert!(
            body.contains("the row showing the account balance"),
            "got {body}"
        );
        assert!(body.contains("claude-code"));
    }

    /// The escalation this gate exists to stop, said in words a person can act on.
    #[test]
    fn the_prompt_explains_why_this_is_a_privilege() {
        let body = describe(&request(false));
        assert!(body.contains("Screen Recording"), "got {body}");
        assert!(
            body.contains("take it back from the menu bar"),
            "a grant with no visible way out is one people will not give, got {body}"
        );
    }
}
