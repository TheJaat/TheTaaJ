//! cat: shows files - a program on the disk, not part of the shell.
//!
//! Its arguments are the names, as typed after `cat`; the file service
//! comes from whoever started it. It exits with 1 if any file could not
//! be shown.
//!
//! With no names it copies its standard input to its standard output,
//! if it was given an input: `cat < notes`. "Shows" is only what
//! happens when the output is the screen - `cat a b > both` joins two
//! files, which is where the name comes from (concatenate).

#![no_std]
#![no_main]

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let mut input = taaj::input();
    if arguments.is_empty() && input.is_given() {
        // No files to ask for, so the file service is not needed.
        input.each(|piece| {
            let _ = taaj::write(piece);
        });
        return 0;
    }
    let Some(files) = taaj::file_service() else {
        println!("cat: started without the file service");
        return 2;
    };
    if arguments.is_empty() {
        println!("usage: cat FILE...");
        return 2;
    }
    let mut failed = false;
    for name in &arguments {
        let mut last = b'\n';
        match files.read(name, |piece| {
            let _ = taaj::write(piece);
            last = piece[piece.len() - 1];
        }) {
            // Leave the next line - the prompt - a line of its own.
            // On the screen only: into a file, cat adds nothing to
            // what the files hold, or a copy would not be a copy.
            Ok(size) if size > 0 && last != b'\n' && taaj::output_is_screen() => println!(),
            Ok(_) => {}
            Err(error) => {
                println!("cat: {name}: {}", error.describe());
                failed = true;
            }
        }
    }
    i64::from(failed)
}
