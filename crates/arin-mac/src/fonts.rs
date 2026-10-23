//! The fonts the person installed, for choosing a hand to write in.
//!
//! CoreText rather than a walk of the font folders, because a font manager such as Adobe
//! Fonts registers fonts that sit in neither `~/Library/Fonts` nor `/Library/Fonts`, and
//! CoreText is the one place that knows about every font an application can use.

use arin_core::hand::FontFile;
use objc2_core_foundation::CFURL;
use objc2_core_text::CTFontManagerCopyAvailableFontURLs;
use std::path::PathBuf;

/// Every font file available to applications, marked with whether macOS shipped it.
///
/// Anything under `/System` came with macOS. Those include a dozen script and handwriting
/// faces, which is why the daemon never picks one of them on its own.
pub fn available_fonts() -> Vec<FontFile> {
    // SAFETY: a plain query with no arguments, returning an owned array of URLs.
    let urls = unsafe { CTFontManagerCopyAvailableFontURLs() };
    // SAFETY: CoreText documents the array's elements as CFURLs.
    let urls = unsafe { urls.cast_unchecked::<CFURL>() };
    let mut paths: Vec<PathBuf> = urls.iter().filter_map(|url| url.to_file_path()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| FontFile {
            from_system: path.starts_with("/System"),
            path,
        })
        .collect()
}
