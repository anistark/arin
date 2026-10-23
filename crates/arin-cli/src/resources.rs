//! Data files shipped beside the binary rather than inside it.

use std::path::PathBuf;

/// Where the stroke packs are, if this build has them.
pub(crate) fn stroke_packs() -> Option<PathBuf> {
    find("strokes", "strokes")
}

/// Where the handwriting fonts for other scripts are, if this build has them.
pub(crate) fn script_fonts() -> Option<PathBuf> {
    find("fonts/scripts", "fonts/scripts")
}

/// A data directory: inside the app bundle when running from one, beside the binary under
/// `share/arin` for a Unix style install, and in the source tree for a build run from a
/// clone. The data is under licences of its own, which is why it is never compiled in.
fn find(installed: &str, source: &str) -> Option<PathBuf> {
    let binary = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok());
    let beside = binary.as_ref().and_then(|b| b.parent()).map(|dir| {
        [
            dir.join("../Resources").join(installed),
            dir.join("../share/arin").join(installed),
        ]
    });
    let tree = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(source);
    beside
        .into_iter()
        .flatten()
        .chain(std::iter::once(tree))
        .find(|dir| dir.is_dir())
}
