//! given: says what it was started with.
//!
//! A program can reach only what it was handed. This one reports what
//! that is - so the shell's choices can be seen: run by its name it has
//! the screen and its arguments; run as `with files given`, the file
//! service as well - and `with readonly given`, the file service by the
//! channel that cannot change anything. With the files comes the
//! directory it was started in, if that is not the root.
//!
//! The files are not handed over as a channel: the program is given a
//! channel to the registry on which the name "files" exists, and asks.
//! So it also says which names it found there.

#![no_std]
#![no_main]

use core::fmt::Write;

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    // The names that exist for this program at the registry - all of
    // them: there is no asking for one that is not here.
    if let Some(Ok(names)) = taaj::services().map(|services| services.list()) {
        println!("given: it may ask for: {}", names.join(" "));
    }
    match taaj::file_service() {
        Some(files) => match files.list("/", |_, _, _| {}) {
            Ok(count) => {
                let how = match files.read_only() {
                    Ok(true) => "to read only",
                    Ok(false) => "to read and to change",
                    Err(_) => "for what, it would not say",
                };
                println!("given: the screen, and the files ({count} of them), {how}");
                if !files.directory().is_empty() {
                    println!("given: started in /{}", files.directory());
                }
            }
            Err(error) => println!("given: the screen, and the files - but {}", error.describe()),
        },
        None => println!("given: the screen, no files"),
    }
    if !arguments.is_empty() {
        // Each in quotes, so that where one ends and the next begins -
        // and an argument that is empty - can be seen.
        let mut text = taaj::Buffer::<400>::new();
        for argument in &arguments {
            let _ = write!(text, " \"{argument}\"");
        }
        println!("given: arguments{}", core::str::from_utf8(text.as_bytes()).unwrap_or(" (not shown)"));
    }
    0
}
