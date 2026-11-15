//! What `write` and `append` share: both store one line of text in a
//! file, and differ only in where it goes. (Not a program: cargo builds
//! the files directly in src/bin, and each of the two includes this.)

use alloc::string::String;

use taaj::println;

/// Runs the command: its arguments are "FILE TEXT". TEXT is stored as
/// a line - replacing the file's contents, or added to their end.
/// Returns the exit code.
pub fn store(command: &str, append: bool) -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        println!("{command}: started without the file service");
        return 2;
    };
    let Some((name, text)) = arguments.trim().split_once(' ') else {
        println!("usage: {command} FILE TEXT");
        return 2;
    };
    let mut line = String::from(text.trim());
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
