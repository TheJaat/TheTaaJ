//! cat: shows files - a program on the disk, not part of the shell.
//!
//! Its arguments are the names, as typed after `cat`; the file service
//! comes from whoever started it. It exits with 1 if any file could not
//! be shown.

#![no_std]
#![no_main]

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        println!("cat: started without the file service");
        return 2;
    };
    if arguments.trim().is_empty() {
        println!("usage: cat FILE...");
        return 2;
    }
    let mut failed = false;
    for name in arguments.split_whitespace() {
        let mut last = b'\n';
        match files.read(name, |piece| {
            let _ = taaj::write(piece);
            last = piece[piece.len() - 1];
        }) {
            // Leave the next line - the prompt - a line of its own.
            Ok(size) if size > 0 && last != b'\n' => println!(),
            Ok(_) => {}
            Err(error) => {
                println!("cat: {name}: {}", error.describe());
                failed = true;
            }
        }
    }
    i64::from(failed)
}
