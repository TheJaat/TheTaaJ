//! What `write` and `append` share: both store one line of text in a
//! file, and differ only in where it goes. (Not a program: cargo builds
//! the files directly in src/bin, and each of the two includes this.)

use alloc::string::String;

use taaj::println;

/// Runs the command: its arguments are FILE and TEXT. TEXT is stored
/// as a line - replacing the file's contents, or added to their end.
/// More than one word of TEXT is stored with one space between each,
/// as `echo` would print them; to keep spaces as typed, quote the text.
/// Returns the exit code.
pub fn store(command: &str, append: bool) -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        println!("{command}: started without the file service");
        return 2;
    };
    // The first argument and the rest - and there must be a rest.
    let Some((name, text)) = arguments.split_first().filter(|(_, text)| !text.is_empty()) else {
        println!("usage: {command} FILE TEXT");
        return 2;
    };
    let mut line: String = text.join(" ");
    line.push('\n');
    let stored = if append { files.append(name, line.as_bytes()) } else { files.write(name, line.as_bytes()) };
    match stored {
        Ok(size) => {
            println!("{name}: {size} bytes");
            0
        }
        Err(error) => {
            println!("{command}: {name}: {}", error.describe());
            1
        }
    }
}
