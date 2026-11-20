//! rm: removes files - a program on the disk, not part of the shell.
//!
//! `rm FILE...`. It exits with 1 if any of them could not be removed.
//! It can remove only what the file service it was handed lets it: the
//! same files the shell that started it could have removed itself.

#![no_std]
#![no_main]

use taaj::eprintln;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        eprintln!("rm: started without the file service");
        return 2;
    };
    if arguments.is_empty() {
        eprintln!("usage: rm FILE...");
        return 2;
    }
    let mut failed = false;
    for name in &arguments {
        if let Err(error) = files.remove(name) {
            eprintln!("rm: {name}: {}", error.describe());
            failed = true;
        }
    }
    i64::from(failed)
}
