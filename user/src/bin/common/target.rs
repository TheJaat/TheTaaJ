//! What `mv` and `cp` share: working out where the thing is to go.
//! (Not a program: each of the two includes this file.)

use alloc::format;
use alloc::string::String;

use taaj::files::{Files, Kind};

/// Where `from` goes when the user says "to `to`". If `to` is a
/// directory, that means into it, under the name it has now: `mv notes
/// docs` makes docs/notes. Anything else - a name that is free, or
/// taken by a file - is the new path itself, and the file service says
/// what it thinks of it.
pub fn target(files: &Files, from: &str, to: &str) -> String {
    match files.stat(to) {
        Ok((Kind::Directory, _)) => {
            // The last name of the whole path, so that "." and "../x"
            // have one: `rsplit` walks the pieces from the right, and
            // the first it gives is the last name.
            let whole = files.resolve(from);
            let name = whole.rsplit('/').next().unwrap_or("");
            format!("{}/{name}", to.trim_end_matches('/'))
        }
        _ => String::from(to),
    }
}
