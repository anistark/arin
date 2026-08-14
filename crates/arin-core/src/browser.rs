//! What a browser has open, read off its own session files.
//!
//! The only layer that sees a **background** tab: a window title reaches the active tab and
//! nothing behind it, so an inbox two tabs deep in another profile is invisible without
//! this. See `plan/TARGETING.md`.
//!
//! # Why files rather than Apple Events
//!
//! Chrome's scripting interface answers this live, and costs an Automation grant that cannot
//! be narrowed to a read: it is "control Google Chrome", carrying tab closing, navigation
//! and JavaScript execution. Holding a write capability to perform a read is the wrong trade
//! here. A file the user already owns needs no grant and can only ever be read.
//!
//! # What that costs
//!
//! Accuracy. A session file is Chrome's crash-recovery journal, not a live view: it lags, it
//! describes the last state written rather than the state now, and closed tabs stay in it
//! until it is rewritten. Every one of those errs towards reporting a tab that is not there,
//! which costs a browser raised for nothing. Never treat a hit as proof.
//!
//! # Privacy
//!
//! Titles and URLs of every open tab is the most sensitive thing Arin reads, which is why it
//! is off unless the user asked. See [`crate::Config::read_browser_tabs`]. Nothing read here
//! may reach a client: it is matched against inside the daemon, and the answer is which
//! application to raise.
//!
//! # Format
//!
//! SNSS. Verified against Chrome 141: a `SNSS` magic and an `i32` version, then records of
//! `[u16 size][u8 command][payload]`. Payloads come in two shapes, and confusing them is the
//! mistake this module has already made for you: [`UPDATE_TAB_NAVIGATION`] is a
//! `base::Pickle` starting with a `u32` byte count, while [`SELECTED_NAVIGATION_INDEX`] and
//! [`TAB_CLOSED`] are raw structs starting at offset zero.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A tab a browser's session file says is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTab {
    /// The profile's display name, such as `"Your Chrome"`.
    ///
    /// Kept because raising the browser cannot switch profiles: an application is the unit
    /// activation works in, so the only useful thing left to say is which window to look in.
    pub profile: String,
    /// The tab's title as of the last navigation Chrome recorded.
    pub title: String,
    /// Its URL.
    pub url: String,
}

/// How stale a session file may be before its profile is assumed closed.
///
/// Chrome writes on navigation and tab changes, so a profile nobody is using goes quiet
/// while its files stay on disk forever. Without a bound, "what is open" quietly becomes
/// "everything this machine has ever had open", across profiles the user may not think of
/// as theirs and may not want an agent reasoning about.
///
/// An hour, measured rather than guessed. On the machine this was built against, the two
/// profiles with windows on screen had been written 42 seconds and 10 minutes earlier,
/// while a profile whose window had been closed for around seven hours was still on disk
/// with a full tab list. An hour separates those cleanly with room to spare.
///
/// Erring short is the safe direction. Missing a profile costs a fallback to asking the
/// user; including a dead one means reasoning about pages nobody has open.
pub const SESSION_CONSIDERED_LIVE: Duration = Duration::from_secs(60 * 60);

/// `kCommandUpdateTabNavigation`. A pickle: tab id, navigation index, url, utf-16 title.
const UPDATE_TAB_NAVIGATION: u8 = 6;
/// `kCommandSetSelectedNavigationIndex`. Raw: tab id, index.
const SELECTED_NAVIGATION_INDEX: u8 = 7;
/// `kCommandTabClosed`. Raw: tab id, then a timestamp this does not need.
const TAB_CLOSED: u8 = 16;

/// Every tab Chrome's session files say is open, across every profile.
///
/// Empty when Chrome is not installed, has never run, or nothing could be parsed. Never an
/// error: this is an optimisation over asking a model, and a caller that cannot have it
/// should carry on without it rather than fail.
pub fn open_tabs() -> Vec<OpenTab> {
    match chrome_root() {
        Some(root) => open_tabs_under(&root),
        None => Vec::new(),
    }
}

/// The same, under an explicit Chrome data directory.
pub fn open_tabs_under(root: &Path) -> Vec<OpenTab> {
    let mut tabs = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return tabs;
    };

    for entry in entries.flatten() {
        let profile = entry.path();
        let Some(session) = newest_session(&profile.join("Sessions")) else {
            continue;
        };
        if is_stale(&session) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&session) else {
            continue;
        };
        let name = profile_name(&profile);
        tabs.extend(
            tabs_in_session(&bytes)
                .into_iter()
                .map(|(title, url)| OpenTab {
                    profile: name.clone(),
                    title,
                    url,
                }),
        );
    }

    tracing::debug!(count = tabs.len(), "tabs read from browser sessions");
    tabs
}

/// Where Chrome keeps its profiles.
fn chrome_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let home = PathBuf::from(home);
    #[cfg(target_os = "macos")]
    let root = home.join("Library/Application Support/Google/Chrome");
    #[cfg(not(target_os = "macos"))]
    let root = home.join(".config/google-chrome");
    root.is_dir().then_some(root)
}

/// The most recently written session file in a profile's `Sessions` directory.
///
/// Chrome keeps the previous session alongside the current one, so the newest is the one
/// describing what is open now.
fn newest_session(sessions: &Path) -> Option<PathBuf> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(sessions).ok()?.flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("Session_"))
        {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(seen, _)| modified > *seen) {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path)
}

fn is_stale(session: &Path) -> bool {
    let Ok(modified) = std::fs::metadata(session).and_then(|m| m.modified()) else {
        return true;
    };
    modified
        .elapsed()
        .is_ok_and(|age| age > SESSION_CONSIDERED_LIVE)
}

/// A profile's display name, falling back to its directory name.
///
/// `Default` and `Profile 13` mean nothing to anybody. "Your Chrome" is what the user sees
/// in the profile switcher, and so the only version worth telling them to look for.
fn profile_name(profile: &Path) -> String {
    let directory = profile
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();

    std::fs::read_to_string(profile.join("Preferences"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|prefs| {
            prefs
                .get("profile")?
                .get("name")?
                .as_str()
                .map(str::to_owned)
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(directory)
}

/// Read one session file into the tabs it says are open.
///
/// Pure, so the format is testable against bytes rather than against somebody's browser.
fn tabs_in_session(bytes: &[u8]) -> Vec<(String, String)> {
    use std::collections::HashMap;

    if bytes.len() < 8 || &bytes[..4] != b"SNSS" {
        return Vec::new();
    }

    // Every navigation seen per tab, the index Chrome says is current, and what was shut.
    let mut navigations: HashMap<i32, HashMap<i32, (String, String)>> = HashMap::new();
    let mut selected: HashMap<i32, i32> = HashMap::new();
    let mut closed: Vec<i32> = Vec::new();

    let mut offset = 8;
    while offset + 2 <= bytes.len() {
        let size = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        // A zero length is the end marker, and a length past the file is a truncated write.
        if size == 0 || offset + 2 + size > bytes.len() {
            break;
        }
        let command = bytes[offset + 2];
        let payload = &bytes[offset + 3..offset + 2 + size];
        offset += 2 + size;

        match command {
            UPDATE_TAB_NAVIGATION => {
                // A pickle, so step over its byte count before anything else.
                if let Some((tab, index, url, title)) = read_navigation(payload) {
                    navigations
                        .entry(tab)
                        .or_default()
                        .insert(index, (title, url));
                }
            }
            SELECTED_NAVIGATION_INDEX => {
                if let (Some(tab), Some(index)) = (read_i32(payload, 0), read_i32(payload, 4)) {
                    selected.insert(tab, index);
                }
            }
            TAB_CLOSED => {
                if let Some(tab) = read_i32(payload, 0) {
                    closed.push(tab);
                }
            }
            _ => {}
        }
    }

    navigations
        .into_iter()
        .filter(|(tab, _)| !closed.contains(tab))
        .filter_map(|(tab, history)| {
            // Where the tab actually is. Chrome records this explicitly; the newest
            // navigation is the fallback when it has not said so yet.
            let current = selected
                .get(&tab)
                .copied()
                .unwrap_or_else(|| history.keys().copied().max().unwrap_or_default());
            history
                .get(&current)
                .or_else(|| history.keys().max().and_then(|last| history.get(last)))
                .cloned()
        })
        .collect()
}

/// Decode a pickled `UpdateTabNavigation` into tab id, index, url and title.
fn read_navigation(payload: &[u8]) -> Option<(i32, i32, String, String)> {
    // Offset 0 is the pickle's own byte count, which nothing here needs.
    let tab = read_i32(payload, 4)?;
    let index = read_i32(payload, 8)?;

    let mut at = 12;
    let url = read_string(payload, &mut at)?;
    let title = read_string16(payload, &mut at).unwrap_or_default();
    Some((tab, index, url, title))
}

fn read_i32(payload: &[u8], at: usize) -> Option<i32> {
    let bytes = payload.get(at..at + 4)?;
    Some(i32::from_le_bytes(bytes.try_into().ok()?))
}

/// A pickled byte string: a length, the bytes, then padding to a four byte boundary.
fn read_string(payload: &[u8], at: &mut usize) -> Option<String> {
    let length = read_i32(payload, *at)?.try_into().ok()?;
    let start = *at + 4;
    let bytes = payload.get(start..start + length)?;
    *at = start + aligned(length);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// A pickled utf-16 string: a *character* count, then that many code units, then padding.
fn read_string16(payload: &[u8], at: &mut usize) -> Option<String> {
    let characters: usize = read_i32(payload, *at)?.try_into().ok()?;
    let length = characters.checked_mul(2)?;
    let start = *at + 4;
    let bytes = payload.get(start..start + length)?;
    *at = start + aligned(length);

    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Pickles pad every field out to four bytes.
fn aligned(length: usize) -> usize {
    length.div_ceil(4) * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a session file the way Chrome does, so the parser is tested against the format
    /// rather than against a fixture somebody wrote to match it.
    #[derive(Default)]
    struct Session(Vec<u8>);

    impl Session {
        fn new() -> Self {
            let mut bytes = b"SNSS".to_vec();
            bytes.extend_from_slice(&3i32.to_le_bytes());
            Self(bytes)
        }

        fn record(&mut self, command: u8, payload: &[u8]) {
            let size = (payload.len() + 1) as u16;
            self.0.extend_from_slice(&size.to_le_bytes());
            self.0.push(command);
            self.0.extend_from_slice(payload);
        }

        fn navigation(mut self, tab: i32, index: i32, url: &str, title: &str) -> Self {
            let mut payload = Vec::new();
            payload.extend_from_slice(&0u32.to_le_bytes()); // pickle byte count, unread
            payload.extend_from_slice(&tab.to_le_bytes());
            payload.extend_from_slice(&index.to_le_bytes());

            payload.extend_from_slice(&(url.len() as i32).to_le_bytes());
            payload.extend_from_slice(url.as_bytes());
            payload.resize(payload.len() + aligned(url.len()) - url.len(), 0);

            let utf16: Vec<u16> = title.encode_utf16().collect();
            payload.extend_from_slice(&(utf16.len() as i32).to_le_bytes());
            for unit in &utf16 {
                payload.extend_from_slice(&unit.to_le_bytes());
            }
            let written = utf16.len() * 2;
            payload.resize(payload.len() + aligned(written) - written, 0);

            self.record(UPDATE_TAB_NAVIGATION, &payload);
            self
        }

        fn selected(mut self, tab: i32, index: i32) -> Self {
            let mut payload = tab.to_le_bytes().to_vec();
            payload.extend_from_slice(&index.to_le_bytes());
            self.record(SELECTED_NAVIGATION_INDEX, &payload);
            self
        }

        fn closed(mut self, tab: i32) -> Self {
            let mut payload = tab.to_le_bytes().to_vec();
            payload.extend_from_slice(&0i64.to_le_bytes());
            self.record(TAB_CLOSED, &payload);
            self
        }

        fn finish(self) -> Vec<u8> {
            self.0
        }
    }

    fn urls(tabs: &[(String, String)]) -> Vec<&str> {
        let mut found: Vec<&str> = tabs.iter().map(|(_, url)| url.as_str()).collect();
        found.sort_unstable();
        found
    }

    #[test]
    fn a_single_tab_reads_back() {
        let session = Session::new()
            .navigation(1, 0, "https://mail.google.com/mail/u/0/", "Inbox - Gmail")
            .finish();

        let tabs = tabs_in_session(&session);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].0, "Inbox - Gmail");
        assert_eq!(tabs[0].1, "https://mail.google.com/mail/u/0/");
    }

    /// The whole reason this layer exists: a background tab is in here, and is in no
    /// window's title.
    #[test]
    fn every_tab_is_read_not_just_the_frontmost() {
        let session = Session::new()
            .navigation(1, 0, "https://example.com/one", "One")
            .navigation(2, 0, "https://example.com/two", "Two")
            .navigation(3, 0, "https://example.com/three", "Three")
            .finish();

        assert_eq!(
            urls(&tabs_in_session(&session)),
            [
                "https://example.com/one",
                "https://example.com/three",
                "https://example.com/two"
            ]
        );
    }

    /// A tab carries its whole navigation history, and reporting the first entry would
    /// describe where the user went ten minutes ago.
    #[test]
    fn a_tab_reports_where_it_is_now_rather_than_where_it_has_been() {
        let session = Session::new()
            .navigation(1, 0, "https://example.com/start", "Start")
            .navigation(1, 1, "https://example.com/middle", "Middle")
            .navigation(1, 2, "https://example.com/now", "Now")
            .selected(1, 2)
            .finish();

        let tabs = tabs_in_session(&session);
        assert_eq!(tabs.len(), 1, "one tab, not one per navigation");
        assert_eq!(tabs[0].1, "https://example.com/now");
    }

    /// Chrome records the selected index explicitly, and it is not always the newest: going
    /// back leaves later entries in the journal.
    #[test]
    fn the_selected_navigation_wins_over_the_newest_one() {
        let session = Session::new()
            .navigation(1, 0, "https://example.com/first", "First")
            .navigation(1, 1, "https://example.com/second", "Second")
            .selected(1, 0)
            .finish();

        assert_eq!(tabs_in_session(&session)[0].1, "https://example.com/first");
    }

    /// Verified against a real file: a closed tab stays in the journal. Reporting it sends
    /// the user to a browser for something they shut.
    #[test]
    fn a_closed_tab_is_not_open() {
        let session = Session::new()
            .navigation(1, 0, "https://example.com/kept", "Kept")
            .navigation(2, 0, "https://example.com/shut", "Shut")
            .closed(2)
            .finish();

        assert_eq!(
            urls(&tabs_in_session(&session)),
            ["https://example.com/kept"]
        );
    }

    /// Titles are utf-16 on disk and people put emoji in page titles.
    #[test]
    fn a_title_survives_being_utf16() {
        let session = Session::new()
            .navigation(1, 0, "https://example.com", "Inbox (12) 📥 — ✅ done")
            .finish();

        assert_eq!(tabs_in_session(&session)[0].0, "Inbox (12) 📥 — ✅ done");
    }

    /// Strings are padded to four bytes, so a length that is not a multiple of four puts
    /// every later field out of place if the padding is not stepped over.
    #[test]
    fn fields_stay_aligned_whatever_the_lengths_are() {
        for url in ["a", "ab", "abc", "abcd", "abcde"] {
            for title in ["x", "xy", "xyz", "wxyz"] {
                let session = Session::new().navigation(7, 0, url, title).finish();
                let tabs = tabs_in_session(&session);
                assert_eq!(tabs.len(), 1, "{url:?}/{title:?} did not parse");
                assert_eq!(tabs[0].1, url, "url wrong for {url:?}/{title:?}");
                assert_eq!(tabs[0].0, title, "title wrong for {url:?}/{title:?}");
            }
        }
    }

    /// A session file is written while Chrome is running, so a read can catch a partial
    /// record. Whatever parsed before it is still worth having.
    #[test]
    fn a_truncated_file_gives_up_where_it_stops_rather_than_panicking() {
        let whole = Session::new()
            .navigation(1, 0, "https://example.com/one", "One")
            .navigation(2, 0, "https://example.com/two", "Two")
            .finish();

        for cut in 8..whole.len() {
            let tabs = tabs_in_session(&whole[..cut]);
            assert!(tabs.len() <= 2, "invented a tab at {cut} bytes");
        }
    }

    #[test]
    fn anything_that_is_not_a_session_file_is_empty_rather_than_an_error() {
        assert!(tabs_in_session(b"").is_empty());
        assert!(tabs_in_session(b"not a session file at all").is_empty());
        assert!(tabs_in_session(&[0u8; 64]).is_empty());
    }

    #[test]
    fn a_missing_chrome_directory_is_no_tabs() {
        assert!(open_tabs_under(Path::new("/nonexistent/chrome")).is_empty());
    }
}
