//! wc: counts lines, words and bytes - a program on the disk, not part
//! of the shell. `wc FILE...`, or `wc < FILE`, or anything else that
//! gives it a standard input.
//!
//! A line is counted at each newline, as Unix counts them: text that
//! does not end in one has a last line that is not counted. A word is
//! a run of characters with no space, tab or newline in it.

#![no_std]
#![no_main]

use taaj::{eprintln, println};

taaj::program!(main);

/// What has been counted so far. Text arrives in pieces that end
/// anywhere - in the middle of a word, as likely as not - so whether
/// the last piece ended inside a word is kept from one to the next.
#[derive(Default)]
struct Count {
    lines: u64,
    words: u64,
    bytes: u64,
    in_word: bool,
}

impl Count {
    fn add(&mut self, piece: &[u8]) {
        self.bytes += piece.len() as u64;
        for &byte in piece {
            if byte == b'\n' {
                self.lines += 1;
            }
            let space = byte == b' ' || byte == b'\t' || byte == b'\n' || byte == b'\r';
            // A word is counted where it begins.
            if !space && !self.in_word {
                self.words += 1;
            }
            self.in_word = !space;
        }
    }

    fn show(&self, name: Option<&str>) {
        let plural = |n: u64| if n == 1 { "" } else { "s" };
        let (lines, words, bytes) = (self.lines, self.words, self.bytes);
        let counts = format_args!("{lines} line{}, {words} word{}, {bytes} byte{}", plural(lines), plural(words), plural(bytes));
        match name {
            Some(name) => println!("{name}: {counts}"),
            None => println!("{counts}"),
        }
    }
}

fn main() -> i64 {
    let arguments = taaj::arguments();
    let mut input = taaj::input();
    if arguments.is_empty() && input.is_given() {
        let mut count = Count::default();
        input.each(|piece| count.add(piece));
        count.show(None);
        return 0;
    }
    let Some(files) = taaj::file_service() else {
        eprintln!("wc: started without the file service");
        return 2;
    };
    if arguments.is_empty() {
        eprintln!("usage: wc FILE...  or  wc < FILE");
        return 2;
    }
    let mut failed = false;
    for name in &arguments {
        let mut count = Count::default();
        match files.read(name, |piece| count.add(piece)) {
            Ok(_) => count.show(Some(name)),
            Err(error) => {
                eprintln!("wc: {name}: {}", error.describe());
                failed = true;
            }
        }
    }
    i64::from(failed)
}
