//! Helpers shared by the tests that run the real restic (`restic_*`, `#[ignore]`d without it).

/// A path as restic reports it in `backup --dry-run -vv --json` (`verbose_status.item`), in the
/// engine's own spelling (`/` separators, `C:/…` for a drive).
///
/// restic prints the path *in the snapshot*, and on Windows a snapshot has no drive letters: the
/// volume becomes the first folder, so `C:\Users\me` is reported as `/C/Users/me`. Unix paths are
/// the same inside and outside the snapshot. A backslash form is accepted too, so the helper holds
/// whichever of the two a restic release prints.
pub(crate) fn restic_item_path(item: &str, windows: bool) -> String {
    if !windows {
        return item.to_string();
    }
    let s = item.replace('\\', "/");
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/') {
        return format!("{}:{}", &s[1..2], if b.len() == 2 { "/" } else { &s[2..] });
    }
    s
}

/// Does the `restic` on `PATH` apply excludes to the paths named on the command line (restic < 0.19)?
/// The differential tests set `judge_sources` from this, so each CI matrix entry is checked against
/// the rules of the restic it runs. Read once.
pub(crate) fn restic_judges_sources() -> bool {
    use std::sync::OnceLock;
    static JUDGES: OnceLock<bool> = OnceLock::new();
    *JUDGES.get_or_init(|| {
        std::process::Command::new("restic")
            .arg("version")
            .output()
            .map(|o| super::plan_tree::restic_version_judges_sources(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::restic_item_path;

    #[test]
    fn unix_paths_are_reported_as_they_are() {
        assert_eq!(restic_item_path("/home/u/a b/", false), "/home/u/a b/");
        // Even something that looks like a volume: on Unix `/c/x` is just a folder called c.
        assert_eq!(restic_item_path("/c/x", false), "/c/x");
    }

    #[test]
    fn a_windows_snapshot_path_puts_the_drive_back() {
        assert_eq!(restic_item_path("/C/Users/me/a b", true), "C:/Users/me/a b");
        assert_eq!(restic_item_path("/C/Users/me/", true), "C:/Users/me/");
        assert_eq!(restic_item_path("/d/data", true), "d:/data");
        assert_eq!(restic_item_path("/C", true), "C:/");
        assert_eq!(restic_item_path("/C/", true), "C:/");
    }

    #[test]
    fn a_native_windows_path_is_only_slash_normalised() {
        assert_eq!(restic_item_path(r"C:\Users\me\a", true), "C:/Users/me/a");
        assert_eq!(restic_item_path("C:/Users/me", true), "C:/Users/me");
    }

    #[test]
    fn a_first_folder_longer_than_a_letter_is_not_a_drive() {
        assert_eq!(restic_item_path("/Users/me", true), "/Users/me");
        assert_eq!(restic_item_path("/", true), "/");
    }
}
