//! Writing a screenshot to disk.
//!
//! It goes where the person's own screenshots go. macOS keeps that folder in the
//! `com.apple.screencapture` preferences, which is what the Screenshot app's Options menu
//! changes, and Arin reads it rather than holding an opinion of its own. Without one set,
//! that is the Desktop, as it is for the system's own shots.

use crate::capture::Image;
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_core_foundation::{CFPreferencesCopyAppValue, CFString, CGSize};
use objc2_foundation::{NSDate, NSDateFormatter, NSDictionary, NSLocale, NSString};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// How many names to try when a shot taken in the same second already has the first one.
const ATTEMPTS: u32 = 100;

/// When the shot was taken, in the form its file name carries.
///
/// Read at the moment of the click, on the main thread, rather than once the capture comes
/// back, so two quick shots are named in the order they were taken.
pub(crate) fn timestamp() -> String {
    let formatter = NSDateFormatter::new();
    // Fixed rather than the person's locale, so the digits are always ones a file name
    // sorts by, whatever calendar and numbering system the Mac is set to.
    formatter.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(
        &NSString::from_str("en_US_POSIX"),
    )));
    formatter.setDateFormat(Some(&NSString::from_str("yyyy-MM-dd 'at' HH.mm.ss")));
    formatter.stringFromDate(&NSDate::now()).to_string()
}

/// The file name for a shot, and for the `attempt`th one taken in the same second.
///
/// The system's own shots are called `Screenshot ...`. Saying Arin in the name keeps the
/// two apart in a folder they share.
fn file_name(stamp: &str, attempt: u32) -> String {
    if attempt <= 1 {
        format!("Arin Screenshot {stamp}.png")
    } else {
        format!("Arin Screenshot {stamp} ({attempt}).png")
    }
}

/// Where the person keeps their screenshots.
fn folder() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let chosen = CFPreferencesCopyAppValue(
        &CFString::from_static_str("location"),
        &CFString::from_static_str("com.apple.screencapture"),
    )
    .and_then(|value| value.downcast::<CFString>().ok())
    .map(|value| value.to_string());

    if let Some(chosen) = chosen
        && let Some(path) = expand(&chosen, home.as_deref())
        && path.is_dir()
    {
        return path;
    }
    match home {
        Some(home) => home.join("Desktop"),
        None => std::env::temp_dir(),
    }
}

/// A folder as the preference stores it, with a leading `~` resolved against `home`.
///
/// The Screenshot app writes an absolute path, but `defaults write` takes whatever it is
/// given, and a tilde is what people type.
fn expand(path: &str, home: Option<&Path>) -> Option<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    if path == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home.map(|home| home.join(rest));
    }
    Some(PathBuf::from(path))
}

/// Encode a captured image as a PNG, at the size it was on screen.
///
/// The size is set in points so a Retina shot carries 144 dpi, as the system's own do,
/// and opens in Preview at the size it was rather than at double.
fn png(image: &Image, points: CGSize) -> Result<Vec<u8>, String> {
    let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image.cg());
    bitmap.setSize(points);
    // SAFETY: an empty dictionary holds no values, so none of them can be the wrong type.
    let data = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or_else(|| "the image could not be encoded as a PNG".to_owned())?;
    Ok(data.to_vec())
}

/// Write a shot into the screenshots folder and say where it went.
///
/// Never replaces a file. A name already taken moves on to the next one, and the check
/// and the write are one step, so two shots in the same second cannot land on each other.
pub(crate) fn write(image: &Image, points: CGSize, stamp: &str) -> Result<PathBuf, String> {
    let bytes = png(image, points)?;
    let folder = folder();
    for attempt in 1..=ATTEMPTS {
        let path = folder.join(file_name(stamp, attempt));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&bytes)
                    .map_err(|e| format!("could not write {}: {e}", path.display()))?;
                return Ok(path);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                return Err(refused(&folder));
            }
            Err(e) => return Err(format!("could not create {}: {e}", path.display())),
        }
    }
    Err(format!(
        "{} already holds {ATTEMPTS} screenshots taken at {stamp}",
        folder.display()
    ))
}

/// What to say when macOS will not let Arin write into the folder.
///
/// The Desktop, Documents and Downloads are folders macOS protects, and the first save into
/// one asks the person whether Arin may. A refusal there is the likeliest reason a save
/// fails, and the fix is in System Settings rather than anywhere in Arin.
fn refused(folder: &Path) -> String {
    format!(
        "macOS did not let Arin save into {}. Allow it in System Settings under Privacy \
         and Security, Files and Folders, or choose another folder in the Screenshot app's \
         Options.",
        folder.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_shot_in_the_same_second_gets_its_own_name() {
        let stamp = "2026-11-17 at 14.03.22";
        assert_eq!(
            file_name(stamp, 1),
            "Arin Screenshot 2026-11-17 at 14.03.22.png"
        );
        assert_eq!(
            file_name(stamp, 2),
            "Arin Screenshot 2026-11-17 at 14.03.22 (2).png"
        );
    }

    #[test]
    fn a_tilde_is_the_home_folder() {
        let home = Path::new("/Users/someone");
        assert_eq!(
            expand("~/Pictures/Shots", Some(home)),
            Some(PathBuf::from("/Users/someone/Pictures/Shots"))
        );
        assert_eq!(expand("~", Some(home)), Some(home.to_path_buf()));
        assert_eq!(expand("~/Shots", None), None);
    }

    #[test]
    fn an_absolute_folder_is_taken_as_it_is() {
        assert_eq!(
            expand(" /Volumes/Shots ", None),
            Some(PathBuf::from("/Volumes/Shots"))
        );
        assert_eq!(expand("", None), None);
    }

    #[test]
    fn the_timestamp_sorts_as_text() {
        let stamp = timestamp();
        assert_eq!(stamp.len(), "2026-11-17 at 14.03.22".len(), "{stamp}");
        assert!(stamp.contains(" at "), "{stamp}");
    }
}
