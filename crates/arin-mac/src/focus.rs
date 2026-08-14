//! Bringing an application's windows to the front.
//!
//! `NSRunningApplication.activateWithOptions`: the call the Dock makes, available to any
//! process with no grant, and one the window server may refuse. Arin holds nothing here a
//! client could not do itself.
//!
//! **Activate is the only verb.** Moving, resizing, closing or arranging a window needs
//! `AXUIElement` and the Accessibility grant Arin promises never to hold, which
//! `plan/SECURITY.md` leans on and CI greps for. Nothing here posts an input event either.
//!
//! It is off unless the user turned it on, because raising an application takes their
//! keyboard with it: somebody typing finishes their sentence in whatever came forward. See
//! `arin_core::Config::allow_activation`.
//!
//! The unit is an application, since choosing between its windows needs Accessibility. A
//! browser is one application holding many profile windows, so a client wanting a
//! particular one gets the application raised and no more.
//!
//! # What is read, and what is reported
//!
//! Matching consults application names, then window titles, then the browser tab index when
//! `--read-browser-tabs` is on. Only the application to raise is ever reported: no title,
//! no URL, no count, no list. That is not tidiness. The candidate list comes from the window
//! list, which needs the Screen Recording grant a client lacks, so an error naming its
//! matches would launder screen contents through Arin's permission, and one listing them all
//! would inventory the desk from a single call. `a_refusal_never_names_an_application_the_user_has_open`
//! and `a_refusal_never_quotes_a_window_title` hold the line.
//!
//! One bit survives: an ambiguous refusal says there was more than one. Collapsing it into
//! the no-match message would close even that, at the cost of an error that cannot tell
//! somebody to be more specific.

use arin_core::{Error, Result};
use arin_protocol::Activated;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

/// Brings applications forward on macOS.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacFocus {
    /// Whether to consult the browser's own session files when nothing else matched.
    ///
    /// Carried here rather than read from configuration, because the seam is a trait with
    /// no access to the daemon. The binary that wires this up passes what the user asked
    /// for, and `false` means the tab index is never built and no session file is opened.
    read_browser_tabs: bool,
}

impl MacFocus {
    /// A focus backend that matches only what the window server can see.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same, also matching against the browser's open tabs.
    ///
    /// Only for a daemon started with `--read-browser-tabs`. See
    /// `arin_core::Config::read_browser_tabs` for what that costs.
    pub fn reading_browser_tabs(read_browser_tabs: bool) -> Self {
        Self { read_browser_tabs }
    }
}

impl arin_core::Focus for MacFocus {
    fn activate(&self, app: &str) -> Result<Activated> {
        let candidates = crate::capture::windowed_applications()?;
        let (wanted, profile) = match pick(app, &candidates) {
            Ok(found) => (found, None),
            // Only when the window server had no answer, and only when the user asked for
            // it. A background tab is the last place worth looking and the most expensive
            // to look in, in what it reads rather than in what it costs to run.
            Err(no_window) if self.read_browser_tabs => {
                let tabs = arin_core::browser::open_tabs();
                match tab_owner(app, &candidates, &tabs) {
                    Some((found, profile)) => (found, Some(profile)),
                    None => return Err(no_window),
                }
            }
            Err(no_window) => return Err(no_window),
        };

        // By pid rather than bundle identifier. Two copies of the same application run with
        // the same bundle id and different windows, and the one worth raising is the one
        // whose window was actually seen.
        let running = NSRunningApplication::runningApplicationWithProcessIdentifier(wanted.pid)
            .ok_or_else(|| {
                Error::Focus(format!(
                    "{} was holding a window a moment ago and is no longer running",
                    wanted.name
                ))
            })?;

        // Every window, not just the main one. A client asked for an application because it
        // wants the user to see what is in it, and raising one window of several leaves the
        // rest behind whatever was in front.
        let accepted =
            running.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
        if !accepted {
            return Err(Error::Focus(format!(
                "the window server would not raise {}",
                wanted.name
            )));
        }

        // The profile is not logged. It is the one thing here derived from a file rather
        // than from the window server, and a log line is a copy of it that outlives the
        // request.
        tracing::info!(
            app = %wanted.name,
            pid = wanted.pid,
            from_tab_index = profile.is_some(),
            "activated"
        );
        Ok(Activated {
            app: wanted.name.clone(),
            bundle_id: wanted.bundle_id.clone(),
            profile,
        })
    }

    fn is_showing(&self, app: &str) -> Result<bool> {
        Ok(showing(app, &crate::capture::windowed_applications()?))
    }
}

/// Whether a window matching `asked` is on the desktop in front of the user.
///
/// Deliberately only the window tiers. A tab index answers "somewhere in this browser",
/// which cannot become "in front of you": the browser is one application, and a wait that
/// returned true because Chrome is showing would end the moment the user is anywhere in
/// Chrome, including the window they started in.
fn showing(asked: &str, candidates: &[crate::capture::WindowedApp]) -> bool {
    pick(asked, candidates).is_ok_and(|app| app.showing)
}

/// Find which application holds an open tab matching `asked`.
///
/// The last tier, and the only one that sees a tab the user is not looking at. Returns the
/// application to raise and the profile holding it, since raising cannot switch profiles.
/// Several matches is ordinary rather than ambiguous — they usually live in one browser,
/// which is all that gets raised either way.
fn tab_owner<'a>(
    asked: &str,
    candidates: &'a [crate::capture::WindowedApp],
    tabs: &[arin_core::OpenTab],
) -> Option<(&'a crate::capture::WindowedApp, String)> {
    let wanted = asked.trim().to_lowercase();
    if wanted.is_empty() {
        return None;
    }

    let tab = tabs.iter().find(|tab| {
        tab.title.to_lowercase().contains(&wanted) || tab.url.to_lowercase().contains(&wanted)
    })?;

    // The tabs came out of Chrome's own files, so Chrome is what holds them. Matched
    // against the window list rather than assumed, because an application with no window
    // cannot be raised and should not be reported as if it could: a browser that has quit
    // still leaves session files describing everything it had open.
    let browser = candidates.iter().find(|app| {
        app.bundle_id.as_deref() == Some(CHROME) || app.name.eq_ignore_ascii_case("Google Chrome")
    })?;

    Some((browser, tab.profile.clone()))
}

/// The only browser whose session files [`arin_core::browser`] can read.
const CHROME: &str = "com.google.Chrome";

/// How closely a candidate answers what was asked for.
///
/// Ordered, and the order is the whole of the matching policy. A weaker tier is only
/// consulted when every stronger one came back empty, so naming an application exactly can
/// never be beaten by something that merely has those letters in a window title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    /// The application's own name, or its bundle identifier, exactly.
    Identity,
    /// Part of the application's name, such as `code` for Visual Studio Code.
    PartialName,
    /// Part of the title of a window it holds, such as `gmail` for a Chrome window.
    WindowTitle,
}

/// Choose the application a client meant.
///
/// Strongest tier first, and ambiguity at whichever tier answers is an error rather than a
/// guess: a wrong window in front of somebody is worse than a request that failed and said
/// why. Window titles are consulted because the thing a person names is usually not an
/// application — nobody has one called Gmail — and a browser window's title is its *active*
/// tab, which is the ceiling on what this can find.
fn pick<'a>(
    asked: &str,
    candidates: &'a [crate::capture::WindowedApp],
) -> Result<&'a crate::capture::WindowedApp> {
    let wanted = asked.trim().to_lowercase();

    // Every string contains the empty one, so without this an empty name matches the whole
    // desk at the partial tier: one application open would be raised, and several would be
    // refused as ambiguous. The protocol rejects an empty `app` before it reaches here, and
    // this is the same rule kept locally so the matcher is safe to call from anywhere.
    if wanted.is_empty() {
        return Err(Error::Focus(
            "nothing open matches that name, in any application or window title".into(),
        ));
    }

    let tier = |app: &crate::capture::WindowedApp| -> Option<Tier> {
        let name = app.name.to_lowercase();
        let bundle = app.bundle_id.as_deref().unwrap_or_default().to_lowercase();
        if name == wanted || bundle == wanted {
            Some(Tier::Identity)
        } else if name.contains(&wanted) {
            Some(Tier::PartialName)
        } else if app
            .titles
            .iter()
            .any(|title| title.to_lowercase().contains(&wanted))
        {
            Some(Tier::WindowTitle)
        } else {
            None
        }
    };

    for level in [Tier::Identity, Tier::PartialName, Tier::WindowTitle] {
        let hits: Vec<&crate::capture::WindowedApp> = candidates
            .iter()
            .filter(|app| tier(app) == Some(level))
            .collect();

        match hits.as_slice() {
            [] => continue,
            // One answer at this tier, and every stronger tier was empty.
            [only] => return Ok(only),
            // Two applications with exactly this name is possible and there is nothing to
            // choose between them, so take the first rather than refuse something that was
            // unambiguous to the person who asked. Below that tier, several hits mean the
            // word was too loose and guessing would move the wrong window.
            several if level == Tier::Identity => return Ok(several[0]),
            _ => {
                return Err(Error::Focus(
                    "that matches more than one application. Name it exactly, or give its \
                     bundle identifier"
                        .into(),
                ));
            }
        }
    }

    // Neither message names an application, and neither repeats the name that was asked
    // for. The caller wraps these in "could not bring {app:?} forward", which has already
    // said what was asked; see `a_refusal_never_names_an_application_the_user_has_open` for
    // what describing the matches would cost.
    Err(Error::Focus(
        "nothing open matches that name, in any application or window title".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::WindowedApp;

    fn app(name: &str, bundle: &str, pid: i32) -> WindowedApp {
        WindowedApp {
            name: name.into(),
            bundle_id: Some(bundle.into()),
            pid,
            titles: Vec::new(),
            // On the desktop in front of the user unless a test says otherwise, since that
            // is the ordinary case and the interesting one has to be spelled.
            showing: true,
        }
    }

    /// An application whose windows are showing something.
    fn app_showing(name: &str, bundle: &str, pid: i32, titles: &[&str]) -> WindowedApp {
        WindowedApp {
            titles: titles.iter().map(|t| (*t).to_string()).collect(),
            ..app(name, bundle, pid)
        }
    }

    /// The same, on a desktop the user has left.
    fn app_elsewhere(name: &str, bundle: &str, pid: i32, titles: &[&str]) -> WindowedApp {
        WindowedApp {
            showing: false,
            ..app_showing(name, bundle, pid, titles)
        }
    }

    fn desk() -> Vec<WindowedApp> {
        vec![
            app("Slack", "com.tinyspeck.slackmacgap", 101),
            app("Safari", "com.apple.Safari", 102),
            app("Visual Studio Code", "com.microsoft.VSCode", 103),
        ]
    }

    /// The case this tier exists for: no application is called Gmail, and one is showing it.
    fn desk_with_mail() -> Vec<WindowedApp> {
        vec![
            app_showing(
                "Google Chrome",
                "com.google.Chrome",
                201,
                &["Inbox (12) - ani@example.com - Gmail", "New Tab"],
            ),
            app_showing("Slack", "com.tinyspeck.slackmacgap", 202, &["Vibrant Labs"]),
        ]
    }

    #[test]
    fn an_exact_name_wins() {
        assert_eq!(pick("Slack", &desk()).unwrap().pid, 101);
    }

    /// An agent writes what the user said, and people do not capitalise application names.
    #[test]
    fn matching_ignores_case_and_surrounding_space() {
        assert_eq!(pick("  slack ", &desk()).unwrap().pid, 101);
        assert_eq!(pick("SAFARI", &desk()).unwrap().pid, 102);
    }

    #[test]
    fn a_bundle_identifier_is_a_name_too() {
        assert_eq!(pick("com.apple.Safari", &desk()).unwrap().pid, 102);
    }

    /// Nobody says "Visual Studio Code" out loud.
    #[test]
    fn an_unambiguous_partial_name_is_enough() {
        assert_eq!(pick("code", &desk()).unwrap().pid, 103);
    }

    /// The whole point of refusing rather than guessing. Picking one would put an
    /// application the user did not ask for in front of them.
    #[test]
    fn an_ambiguous_partial_name_is_refused_and_says_how_to_narrow_it() {
        let crowded = vec![
            app("Safari", "com.apple.Safari", 102),
            app(
                "Safari Technology Preview",
                "com.apple.SafariTechnology",
                104,
            ),
        ];
        assert!(
            pick("safari tech", &crowded).is_ok(),
            "one hit is not ambiguous"
        );

        let said = pick("saf", &crowded)
            .expect_err("two hits and no way to choose")
            .to_string();
        assert!(said.contains("more than one"), "got {said}");
        assert!(
            said.contains("bundle identifier"),
            "a refusal has to say how to stop being refused, got {said}"
        );
    }

    /// The reason the tier exists. Nobody has an application called Gmail, and asking for
    /// one used to fail with an inbox open in a Chrome window on the next desktop.
    #[test]
    fn a_window_title_finds_what_no_application_is_called() {
        assert_eq!(pick("gmail", &desk_with_mail()).unwrap().pid, 201);
        assert_eq!(pick("Inbox", &desk_with_mail()).unwrap().pid, 201);
    }

    /// A title is the weakest evidence there is, so it must never outrank somebody naming
    /// an application outright. Chrome is showing the word "Slack" in a tab; asking for
    /// Slack has to reach Slack.
    #[test]
    fn naming_an_application_beats_a_window_that_merely_mentions_it() {
        let confusing = vec![
            app_showing(
                "Google Chrome",
                "com.google.Chrome",
                201,
                &["Slack | vibrantlabs | Slack"],
            ),
            app_showing("Slack", "com.tinyspeck.slackmacgap", 202, &["Vibrant Labs"]),
        ];
        assert_eq!(
            pick("slack", &confusing).unwrap().pid,
            202,
            "the application itself wins over a browser tab about it"
        );
    }

    /// Same rule one tier down: a partial name still beats a title.
    #[test]
    fn a_partial_application_name_beats_a_window_title() {
        let confusing = vec![
            app_showing(
                "Google Chrome",
                "com.google.Chrome",
                201,
                &["figma - Board"],
            ),
            app_showing("Figma Desktop", "com.figma.Desktop", 202, &["Untitled"]),
        ];
        assert_eq!(pick("figma", &confusing).unwrap().pid, 202);
    }

    /// Two applications showing the same word is exactly the case where guessing moves the
    /// wrong window.
    #[test]
    fn two_windows_showing_the_same_thing_are_refused() {
        let both = vec![
            app_showing("Google Chrome", "com.google.Chrome", 201, &["Gmail"]),
            app_showing("Safari", "com.apple.Safari", 202, &["Gmail"]),
        ];
        let said = pick("gmail", &both)
            .expect_err("two applications are showing it")
            .to_string();
        assert!(said.contains("more than one"), "got {said}");
        assert!(!said.contains("Chrome"), "the refusal named an app: {said}");
        assert!(!said.contains("Safari"), "the refusal named an app: {said}");
    }

    /// An application with several windows contributes all of their titles, since the one
    /// worth finding is rarely the frontmost.
    #[test]
    fn any_window_of_an_application_can_answer() {
        let many = vec![app_showing(
            "Google Chrome",
            "com.google.Chrome",
            201,
            &["New Tab", "Radiohead - YouTube", "Inbox - Gmail"],
        )];
        assert_eq!(pick("youtube", &many).unwrap().pid, 201);
        assert_eq!(pick("gmail", &many).unwrap().pid, 201);
    }

    /// An exact hit is not ambiguous even when it is also a prefix of something else.
    #[test]
    fn an_exact_name_beats_a_longer_partial_match() {
        let crowded = vec![
            app(
                "Safari Technology Preview",
                "com.apple.SafariTechnology",
                104,
            ),
            app("Safari", "com.apple.Safari", 102),
        ];
        assert_eq!(pick("Safari", &crowded).unwrap().pid, 102);
    }

    /// The message covers both places a name could have matched, so somebody who expected
    /// a window title to answer learns that it was looked at too.
    #[test]
    fn a_name_nobody_has_open_says_so() {
        let error = pick("Linear", &desk_with_mail()).expect_err("not on this desk");
        let said = error.to_string();
        assert!(said.contains("nothing open matches"), "{said}");
        assert!(said.contains("window title"), "{said}");
    }

    /// The daemon wraps these in `could not bring "Linear" forward`, so a message that
    /// named the application again would reach the user saying it twice.
    #[test]
    fn a_refusal_leaves_naming_the_application_to_its_caller() {
        for asked in ["Linear", "saf"] {
            let crowded = vec![
                app("Safari", "com.apple.Safari", 102),
                app(
                    "Safari Technology Preview",
                    "com.apple.SafariTechnology",
                    104,
                ),
            ];
            let said = pick(asked, &crowded)
                .expect_err("neither name resolves to one application")
                .to_string();
            assert!(
                !said.contains(asked),
                "{asked:?} came back in its own refusal: {said}"
            );
        }
    }

    /// The candidate list comes from the window list, which needs Screen Recording.
    #[test]
    fn a_refusal_never_names_an_application_the_user_has_open() {
        // Distinctive enough that a leak cannot hide inside an ordinary English word.
        let private = vec![
            app("Zulip", "com.zulip.desktop", 201),
            app("Zoiper", "com.zoiper.client", 202),
            app("Quicksilver", "com.blacktree.Quicksilver", 203),
        ];

        // Every probe that fails: nothing matching, a substring hitting several, a single
        // letter fishing for the lot, and a bundle-shaped guess.
        for asked in ["Linear", "z", "i", "e", "com.zulip", "quick silver", "  "] {
            let Err(error) = pick(asked, &private) else {
                continue;
            };
            let said = error.to_string();
            for open in &private {
                assert!(
                    !said.contains(&open.name),
                    "asking for {asked:?} named {:?}, which the client had no way to \
                     learn: {said}",
                    open.name
                );
                let bundle = open.bundle_id.as_deref().expect("fixtures carry one");
                assert!(
                    !said.contains(bundle),
                    "asking for {asked:?} named {bundle:?}: {said}"
                );
            }
        }
    }

    /// Window titles are the most sensitive thing this module touches: a document name, a
    /// subject line, a customer's name in a CRM tab. They are read to decide and must never
    /// come back out, which is a stronger rule than the one covering application names.
    #[test]
    fn a_refusal_never_quotes_a_window_title() {
        let private = vec![
            app_showing(
                "Google Chrome",
                "com.google.Chrome",
                201,
                &[
                    "Re: severance terms - confidential - Gmail",
                    "Zoiper pricing",
                ],
            ),
            app_showing(
                "Preview",
                "com.apple.Preview",
                202,
                &["biopsy-results-2026.pdf"],
            ),
        ];

        for asked in ["Linear", "severance", "z", "e", "pdf", "confidential", "  "] {
            let Err(error) = pick(asked, &private) else {
                continue;
            };
            let said = error.to_string();
            for open in &private {
                for title in &open.titles {
                    assert!(
                        !said.contains(title.as_str()),
                        "asking for {asked:?} quoted the window title {title:?}: {said}"
                    );
                }
            }
        }
    }

    // waiting for arrival

    /// The state a wait exists to leave: the window is there, on a desktop the user is not
    /// looking at.
    #[test]
    fn an_application_on_another_desktop_is_not_showing() {
        let desk = vec![app_elsewhere(
            "Google Chrome",
            "com.google.Chrome",
            401,
            &["Inbox - Mail"],
        )];
        assert!(!showing("chrome", &desk));
        assert!(
            !showing("inbox", &desk),
            "a window title reaches it either way"
        );
    }

    #[test]
    fn an_application_in_front_of_the_user_is_showing() {
        let desk = vec![app_showing(
            "Slack",
            "com.tinyspeck.slackmacgap",
            402,
            &["Vibrant Labs"],
        )];
        assert!(showing("slack", &desk));
    }

    /// One window here and one on the desktop they left still counts as in front of them,
    /// which is what a person would say looking at it.
    #[test]
    fn one_window_showing_is_enough() {
        let mut chrome = app_showing("Google Chrome", "com.google.Chrome", 403, &["New Tab"]);
        chrome.titles.push("Inbox - Mail".into());
        assert!(showing("chrome", &[chrome]));
    }

    /// A name nobody has open is not showing, and must not read as arrival. A wait that
    /// ended on an unmatched name would report the user had moved somewhere that does not
    /// exist.
    #[test]
    fn a_name_that_matches_nothing_is_not_showing() {
        let desk = vec![app_showing(
            "Slack",
            "com.tinyspeck.slackmacgap",
            402,
            &["Vibrant Labs"],
        )];
        assert!(!showing("linear", &desk));
        assert!(!showing("   ", &desk));
    }

    /// An ambiguous name resolves to nothing, so it cannot resolve to arrival either.
    #[test]
    fn an_ambiguous_name_is_not_showing() {
        let crowded = vec![
            app_showing("Safari", "com.apple.Safari", 404, &["Start"]),
            app_showing(
                "Safari Technology Preview",
                "com.apple.SafariTech",
                405,
                &["Start"],
            ),
        ];
        assert!(!showing("saf", &crowded));
    }

    // the tab index

    fn tab(profile: &str, title: &str, url: &str) -> arin_core::OpenTab {
        arin_core::OpenTab {
            profile: profile.into(),
            title: title.into(),
            url: url.into(),
        }
    }

    fn browser_desk() -> Vec<WindowedApp> {
        vec![
            app_showing(
                "Google Chrome",
                "com.google.Chrome",
                301,
                &["Usage \u{b7} Settings"],
            ),
            app_showing("Slack", "com.tinyspeck.slackmacgap", 302, &["Vibrant Labs"]),
        ]
    }

    /// The case the whole layer exists for. The tab is not the active one in any window, so
    /// no window title mentions it, and it is still what the user asked about.
    #[test]
    fn a_background_tab_is_found_and_says_which_profile_holds_it() {
        let tabs = [tab(
            "Your Chrome",
            "Inbox - Vibrant Labs Mail",
            "https://mail.google.com/mail/u/0/",
        )];
        let desk = browser_desk();
        let (app, profile) = tab_owner("inbox", &desk, &tabs).expect("open in a tab");
        assert_eq!(app.pid, 301, "the browser holding it is what gets raised");
        assert_eq!(
            profile, "Your Chrome",
            "raising cannot switch profile, so naming the window is the useful part"
        );
    }

    /// A URL is never in a window title, so matching one proves the answer came from the
    /// tab index rather than from anything the window server could have said.
    #[test]
    fn a_url_matches_even_though_no_window_is_titled_with_one() {
        let tabs = [tab(
            "Your Chrome",
            "Inbox - Vibrant Labs Mail",
            "https://mail.google.com/mail/u/0/",
        )];
        let desk = browser_desk();
        assert!(tab_owner("mail.google.com", &desk, &tabs).is_some());
    }

    /// Session files outlive the browser. Reporting a tab in an application that is not
    /// running would raise nothing and tell the user to look at a window that is gone.
    #[test]
    fn a_tab_in_a_browser_with_no_window_is_not_an_answer() {
        let tabs = [tab("Your Chrome", "Inbox", "https://mail.google.com/")];
        let no_browser = vec![app_showing(
            "Slack",
            "com.tinyspeck.slackmacgap",
            302,
            &["Vibrant Labs"],
        )];
        assert!(tab_owner("inbox", &no_browser, &tabs).is_none());
    }

    #[test]
    fn nothing_in_any_tab_is_no_answer_rather_than_a_guess() {
        let tabs = [tab("Your Chrome", "Inbox", "https://mail.google.com/")];
        let desk = browser_desk();
        assert!(tab_owner("figma", &desk, &tabs).is_none());
        assert!(tab_owner("   ", &desk, &tabs).is_none());
    }

    /// Several tabs matching is ordinary rather than ambiguous: they usually live in one
    /// browser, and the browser is the only thing that can be raised either way.
    #[test]
    fn several_matching_tabs_still_raise_the_browser() {
        let tabs = [
            tab("Your Chrome", "Inbox - Mail", "https://mail.google.com/"),
            tab(
                "AniStark",
                "Mail settings",
                "https://mail.google.com/settings",
            ),
        ];
        let desk = browser_desk();
        let (app, _) = tab_owner("mail", &desk, &tabs).expect("both are mail");
        assert_eq!(app.pid, 301);
    }

    /// A count is a weaker version of the same leak: "matches 7" measures the desk without
    /// naming it.
    #[test]
    fn an_ambiguous_refusal_does_not_measure_how_many_it_matched() {
        let many: Vec<WindowedApp> = ["One", "Two", "Three", "Four", "Five", "Six", "Seven"]
            .iter()
            .enumerate()
            .map(|(i, word)| {
                app(
                    &format!("Probe {word}"),
                    &format!("com.probe.{}", word.to_lowercase()),
                    300 + i as i32,
                )
            })
            .collect();

        let said = pick("probe", &many).expect_err("seven hits").to_string();
        assert!(
            !said.chars().any(|c| c.is_ascii_digit()),
            "the number of matches reached the client: {said}"
        );
    }
}
