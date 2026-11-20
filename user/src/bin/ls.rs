//! ls: lists what is in a directory - a program on the disk, not part
//! of the shell. `ls` lists the directory it was started in; `ls PATH...`
//! lists each directory named (and, for a file, shows its line).
//!
//! It can, because whoever starts it hands it a channel to the file
//! service (taaj::file_service). Without one it has no files to list:
//! there is no global file system a program can reach on its own.

#![no_std]
#![no_main]

use taaj::files::{Files, Kind};
use taaj::{eprintln, println};

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        eprintln!("ls: started without the file service");
        return 2;
    };
    let several = arguments.len() > 1;
    let mut failed = false;
    if arguments.is_empty() {
        // "." and not "": if the directory is gone, the message has
        // something to name.
        failed = !show(&files, ".", false);
    }
    for path in &arguments {
        failed |= !show(&files, path, several);
    }
    i64::from(failed)
}

/// Lists the directory `path`, or shows the file it names. Whether it
/// could.
fn show(files: &Files, path: &str, headed: bool) -> bool {
    match files.stat(path) {
        Ok((Kind::File, size)) => {
            println!("{size:>8}  {path}");
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
            println!("{size:>8}  {name}");
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
