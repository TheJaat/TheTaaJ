//! Paths as people type them, turned into paths the file service takes.
//!
//! The service knows one kind of path: every name from the root down,
//! "docs/notes/today", with no "." and no ".." in it - it refuses those.
//! What is typed is looser: "today" while in docs/notes, "../plan",
//! "./given", "/welcome". `resolve` is the step between the two, and it
//! runs in the client - the shell, and every program that takes file
//! names - because "the directory I am in" is the client's to know. The
//! service is never told, and so has nothing to forget when it is
//! restarted.
//!
//! The rules:
//!
//!   - a path that starts with '/' is from the root; any other is from
//!     the directory given;
//!   - "." is "here": it is dropped;
//!   - ".." is "one up": it takes back the name before it. At the root
//!     there is nothing to take back, and it stays at the root - the
//!     root is its own parent, as in Unix;
//!   - empty names (a doubled or a final '/') mean nothing.
//!
//! This is worked out on the text alone: the disk is not asked. So
//! "nosuch/../welcome" is "welcome" whether or not `nosuch` exists.
//! Unix asks the disk at every step, and has to: there a name can be a
//! symbolic link, and ".." after a link is the parent of wherever the
//! link led. TheTaaJ has no links, so the text says all there is.

use alloc::string::String;
use alloc::vec::Vec;

/// The path from the root that `path` means, for someone in `directory`
/// (itself a path from the root; "" is the root). The result has no
/// '/' at either end, no "." and no ".."; "" is the root.
pub fn resolve(directory: &str, path: &str) -> String {
    // Where to start: nowhere for a path from the root, else the names
    // of the directory. `chain` runs through one list of names and then
    // the other, as if they were a single path.
    let from = if path.starts_with('/') { "" } else { directory };
    // Views into the two strings: no name is copied until the join.
    let mut names: Vec<&str> = Vec::new();
    for name in from.split('/').chain(path.split('/')) {
        match name {
            "" | "." => {}
            ".." => {
                names.pop(); // at the root: nothing to pop, and no error
            }
            name => names.push(name),
        }
    }
    names.join("/")
}

/// How a resolved path is shown: with the '/' it is from. "" is "/".
pub fn display(resolved: &str) -> String {
    let mut shown = String::from("/");
    shown.push_str(resolved);
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_in_the_directory() {
        assert_eq!(resolve("", "welcome"), "welcome");
        assert_eq!(resolve("docs", "readme"), "docs/readme");
        assert_eq!(resolve("docs/notes", "a/b"), "docs/notes/a/b");
    }

    #[test]
    fn a_leading_slash_is_from_the_root() {
        assert_eq!(resolve("docs/notes", "/welcome"), "welcome");
        assert_eq!(resolve("docs", "/"), "");
        assert_eq!(resolve("docs", "//tools//given/"), "tools/given");
    }

    #[test]
    fn dot_is_here_and_nothing_is_here_too() {
        assert_eq!(resolve("docs", "."), "docs");
        assert_eq!(resolve("docs", ""), "docs");
        assert_eq!(resolve("docs", "./readme"), "docs/readme");
        assert_eq!(resolve("docs", "a/./b/."), "docs/a/b");
        assert_eq!(resolve("", "."), "");
    }

    #[test]
    fn dot_dot_goes_up() {
        assert_eq!(resolve("docs/notes", ".."), "docs");
        assert_eq!(resolve("docs/notes", "../.."), "");
        assert_eq!(resolve("docs/notes", "../plan"), "docs/plan");
        assert_eq!(resolve("docs", "notes/../readme"), "docs/readme");
        assert_eq!(resolve("docs", "/tools/../welcome"), "welcome");
    }

    #[test]
    fn the_root_is_its_own_parent() {
        assert_eq!(resolve("", ".."), "");
        assert_eq!(resolve("docs", "../../../welcome"), "welcome");
        assert_eq!(resolve("", "/../.."), "");
    }

    #[test]
    fn names_that_only_look_like_dots_are_names() {
        assert_eq!(resolve("", "..."), "...");
        assert_eq!(resolve("docs", ".hidden"), "docs/.hidden");
        assert_eq!(resolve("docs", "a..b/..c"), "docs/a..b/..c");
    }

    #[test]
    fn a_directory_that_is_not_tidy_is_tidied() {
        // The directory comes from whoever started the program: it is
        // resolved like the path, so nothing odd in it survives.
        assert_eq!(resolve("/docs//notes/", "today"), "docs/notes/today");
        assert_eq!(resolve("docs/../tools", "given"), "tools/given");
    }

    #[test]
    fn resolving_twice_changes_nothing() {
        for (directory, path) in [("", "a"), ("docs", "../x/./y"), ("a/b", "/c/../d"), ("a", "..")] {
            let once = resolve(directory, path);
            assert_eq!(resolve("", &once), once);
            assert_eq!(resolve("anywhere", &display(&once)), once);
        }
    }

    #[test]
    fn what_is_resolved_is_what_the_service_takes() {
        // No "." or ".." is left for the service to refuse.
        for path in ["..", "./a/../b", "a/b/../../..", "/x/./y/"] {
            let resolved = resolve("docs/notes", path);
            assert!(resolved.split('/').all(|name| name != "." && name != ".."), "{resolved}");
            assert!(!resolved.starts_with('/') && !resolved.ends_with('/'), "{resolved}");
        }
    }

    #[test]
    fn shown_with_its_slash() {
        assert_eq!(display(""), "/");
        assert_eq!(display("docs/notes"), "/docs/notes");
    }
}
