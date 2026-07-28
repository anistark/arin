//! Daemon configuration.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Runtime settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where the Unix domain socket lives. Created with mode 0600.
    pub socket_path: PathBuf,
    /// How long annotations linger after a session ends or its socket drops.
    pub linger: Duration,
    /// How often to check whether content scrolled, during active sessions only.
    pub scroll_tick: Duration,
    /// How often to sweep for annotations whose time to live has run out.
    ///
    /// This is the granularity of a TTL, not its accuracy: a mark asked to live 500ms
    /// lives 500ms plus up to one tick. Separate from `scroll_tick` because a sweep is a
    /// walk over a handful of annotations while a scroll check is a screen capture, so
    /// there is no reason for the cheap one to run at the expensive one's rate.
    pub expiry_tick: Duration,
    /// Default time to live for an annotation. `None` means it lives until cleared.
    ///
    /// A client's own `ttl_ms` overrides this. Set it to put a ceiling on how long any
    /// mark can sit on screen regardless of what clients ask for.
    pub default_ttl: Option<Duration>,
    /// Reject peers whose effective uid differs from the daemon's.
    pub require_same_uid: bool,
    /// Which registered resolver grounds the query form of `point` and `highlight`.
    ///
    /// `None` leaves the query form refused with `no_resolver`, which is the default and
    /// the only state in which nothing Arin does can reach the network. Naming one is a
    /// deliberate act: a resolver may send screenshots off the machine, so it is never
    /// inferred from a key being present in the environment.
    pub resolver: Option<String>,
    /// Look at the target region before marking it.
    ///
    /// Costs one capture per positioned annotation, and buys two things from it: a colour
    /// that contrasts with what is already there, and a record of that content so a mark
    /// following a scroll can be checked against where it landed. Turn it off to draw
    /// everything in the default colour and never capture except for scroll detection,
    /// accepting that marks are then followed on the display-wide movement alone.
    pub adaptive_color: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket_path: default_socket_path(),
            // Long enough to read a final annotation, short enough that a crashed client
            // does not leave marks on the screen.
            linger: Duration::from_secs(5),
            // Fast enough that a scroll does not leave a stale mark visible for long,
            // slow enough to stay off the CPU graph.
            scroll_tick: Duration::from_millis(500),
            // Fine enough that a short TTL is not visibly wrong, coarse enough to be
            // free. A mark asked to live a second should not linger for a noticeable
            // fraction of another one.
            expiry_tick: Duration::from_millis(100),
            default_ttl: None,
            require_same_uid: true,
            // Nothing grounds and nothing leaves the machine until someone says so.
            resolver: None,
            adaptive_color: true,
        }
    }
}

impl Config {
    /// Config with an explicit socket path.
    pub fn with_socket_path(path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: path.into(),
            ..Self::default()
        }
    }

    /// The directory the socket lives in.
    pub fn socket_dir(&self) -> Option<&Path> {
        self.socket_path.parent()
    }
}

/// Where the socket goes when nothing overrides it.
///
/// Prefers the per-user runtime directory, which is already mode 0700 and cleaned up on
/// logout. Falling back to the temp directory means falling back to somewhere
/// world-writable, so in that case the socket goes inside a per-uid subdirectory that
/// the server creates mode 0700.
pub fn default_socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir.join(format!("{}.sock", crate::APP_NAME)),
        _ => std::env::temp_dir()
            .join(format!(
                "{}-{}",
                crate::APP_NAME,
                crate::peer::current_uid()
            ))
            .join(format!("{}.sock", crate::APP_NAME)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_is_named_after_the_app() {
        let path = default_socket_path();
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("{}.sock", crate::APP_NAME)
        );
        assert!(path.is_absolute());
    }

    #[test]
    fn defaults_are_locked_down() {
        let config = Config::default();
        assert!(config.require_same_uid);
        assert_eq!(config.default_ttl, None);
        assert_eq!(
            config.resolver, None,
            "a default daemon must not be able to send anything off the machine"
        );
    }
}
