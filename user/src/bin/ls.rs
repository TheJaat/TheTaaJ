//! ls: lists what is in a directory - a program on the disk, not part
//! of the shell. `ls` lists the directory it was started in; `ls PATH...`
//! lists each directory named (and, for a file, shows its line).
//! `ls -l` adds when each file last changed, in UTC, on disks that keep
//! times (TaajFS version 6); a file changed before its disk began
//! keeping them shows "-".
//!
//! It can, because whoever starts it hands it a channel to the file
//! service (taaj::file_service). Without one it has no files to list:
//! there is no global file system a program can reach on its own.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use taaj::files::{Files, Kind};
use taaj::{eprintln, println};

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        eprintln!("ls: started without the file service");
        return 2;
    };
    let long = arguments.first().is_some_and(|first| first == "-l");
    let paths = &arguments[usize::from(long)..];
    let several = paths.len() > 1;
    let mut failed = false;
    if paths.is_empty() {
        // "." and not "": if the directory is gone, the message has
        // something to name.
        failed = !show(&files, ".", several, long);
    }
    for path in paths {
        failed |= !show(&files, path, several, long);
    }
    i64::from(failed)
}

/// Lists the directory `path`, or shows the file it names. Whether it
/// could.
fn show(files: &Files, path: &str, headed: bool, long: bool) -> bool {
    match files.stat(path) {
        Ok((Kind::File, size)) => {
            if long {
                println!("{size:>8}  {}  {path}", when(files, path));
            } else {
                println!("{size:>8}  {path}");
            }
            return true;
        }
        Ok((Kind::Directory, _)) => {}
        Err(error) => {
            eprintln!("ls: {path}: {}", error.describe());
            return false;
        }
    }
    if headed {
        println!("{path}:");
    }
    let (mut directories, mut bytes) = (0, 0);
    let listed = files.list(path, |name, kind, size| match kind {
        Kind::Directory => {
            println!("     dir  {name}/");
            directories += 1;
        }
        Kind::File => {
            if long {
                let full = if path == "." { String::from(name) } else { format!("{path}/{name}") };
                println!("{size:>8}  {}  {name}", when(files, &full));
            } else {
                println!("{size:>8}  {name}");
            }
            bytes += size;
        }
    });
    let plural = |n: usize, one: &'static str, many: &'static str| if n == 1 { one } else { many };
    match listed {
        Ok(0) => println!("empty"),
        Ok(count) => {
            if directories > 0 {
                taaj::print!("{directories} director{}, ", plural(directories, "y", "ies"));
            }
            let count = count - directories;
            println!("{count} file{}, {bytes} bytes", plural(count, "", "s"));
        }
        Err(error) => {
            eprintln!("ls: {path}: {}", error.describe());
            return false;
        }
    }
    true
}

/// When the file at `path` last changed, as `YYYY-MM-DD HH:MM:SS` in
/// UTC - or "-", padded to the same width, if its disk has no time for
/// it.
fn when(files: &Files, path: &str) -> String {
    match files.stat_timed(path) {
        Ok((_, _, _, time)) if time != 0 => date(time / 1_000_000_000),
        _ => format!("{:<19}", "-"),
    }
}

/// `seconds` since 1970 as a UTC date and time. The days become a date
/// by Howard Hinnant's days-to-civil reckoning: count in 400-year eras
/// (each exactly 146 097 days), and start each year in March, so that
/// February - the month whose length varies - comes last.
fn date(seconds: u64) -> String {
    let days = seconds / 86_400;
    let second = seconds % 86_400;
    let z = days + 719_468; // days since 0000-03-01
    let era = z / 146_097;
    let of_era = z - era * 146_097; // 0..=146096
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let m = (5 * day_of_year + 2) / 153; // 0 = March
    let day = day_of_year - (153 * m + 2) / 5 + 1;
    let month = if m < 10 { m + 3 } else { m - 9 };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        second / 3600,
        second / 60 % 60,
        second % 60
    )
}
