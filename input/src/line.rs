//! From a line as it was typed to the words in it.
//!
//! The keyboard gives characters; the shell collects them into a line;
//! and a line is a command and its arguments. Where one argument ends
//! and the next begins is decided here, once, by the shell - so that
//! programs get their arguments already separated and never have to
//! guess. (If each program split the line itself, a file name with a
//! space in it could not be given to any of them.)
//!
//! The rules, which are the Unix shell's, as far as they go:
//!
//!   - words are separated by spaces or tabs, any number of them;
//!   - inside 'single quotes' every character is itself, with no
//!     exceptions - so a single quote cannot appear there;
//!   - inside "double quotes" every character is itself except the
//!     backslash, which makes the next character itself: \" is a quote
//!     that does not end the word, \\ is one backslash;
//!   - outside quotes a backslash does the same: a\ b is one word;
//!   - quoted and unquoted pieces that touch are one word: a"b c"d is
//!     the word `ab cd`, and "" is a word with nothing in it.
//!
//! Nothing else is special yet: no variables, no wildcards. A quote
//! that is never closed is an error, not a guess - the line is not run.
//!
//! Pure logic, so it is tested on the host.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Why a line could not be split into words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineError {
    /// A quote - this one - was opened and never closed.
    OpenQuote(char),
    /// The line ends in a backslash, with nothing after it to take.
    TrailingBackslash,
}

impl LineError {
    pub fn describe(self) -> &'static str {
        match self {
            LineError::OpenQuote('\'') => "the line has a ' that is never closed",
            LineError::OpenQuote(_) => "the line has a \" that is never closed",
            LineError::TrailingBackslash => "the line ends in a \\ with nothing after it",
        }
    }
}

/// Where the reader is: between quotes, or inside a pair of them.
#[derive(Clone, Copy, PartialEq)]
enum Within {
    Nothing,
    Single,
    Double,
}

/// The words of `line`, in order. An empty or blank line has none.
pub fn words(line: &str) -> Result<Vec<String>, LineError> {
    let mut words = Vec::new();
    // The word being collected - and whether there is one at all: `""`
    // is a word though it has no characters, so the text alone cannot
    // say. `Option` carries that: None is "between words".
    let mut word: Option<String> = None;
    let mut within = Within::Nothing;
    let mut characters = line.chars();

    // `while let` takes characters until there are none. The loop body
    // may take one more itself - the character after a backslash -
    // which a `for` loop, holding the iterator for itself, would not
    // allow.
    while let Some(c) = characters.next() {
        match (within, c) {
            // Between words: space ends the word, if one was begun.
            (Within::Nothing, ' ' | '\t') => words.extend(word.take()),
            (Within::Nothing, '\'') => {
                within = Within::Single;
                word.get_or_insert_with(String::new);
            }
            (Within::Nothing, '"') => {
                within = Within::Double;
                word.get_or_insert_with(String::new);
            }
            // The closing quote of the kind that was opened.
            (Within::Single, '\'') | (Within::Double, '"') => within = Within::Nothing,
            // A backslash, anywhere but in single quotes: whatever
            // comes next is an ordinary character.
            (Within::Nothing | Within::Double, '\\') => {
                let next = characters.next().ok_or(LineError::TrailingBackslash)?;
                word.get_or_insert_with(String::new).push(next);
            }
            // Everything else is part of the word.
            (_, c) => word.get_or_insert_with(String::new).push(c),
        }
    }
    match within {
        Within::Nothing => {
            words.extend(word);
            Ok(words)
        }
        Within::Single => Err(LineError::OpenQuote('\'')),
        Within::Double => Err(LineError::OpenQuote('"')),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(line: &str) -> Vec<String> {
        words(line).unwrap()
    }

    #[test]
    fn spaces_separate_words() {
        assert_eq!(split("write notes hello"), ["write", "notes", "hello"]);
        assert_eq!(split("  ls \t docs   "), ["ls", "docs"]);
        assert!(split("").is_empty());
        assert!(split("   \t ").is_empty());
    }

    #[test]
    fn quotes_keep_spaces() {
        assert_eq!(split("write notes \"two  words\""), ["write", "notes", "two  words"]);
        assert_eq!(split("write 'my file' x"), ["write", "my file", "x"]);
        assert_eq!(split("\"  \""), ["  "]);
    }

    #[test]
    fn each_kind_of_quote_holds_the_other() {
        assert_eq!(split("echo \"it's\""), ["echo", "it's"]);
        assert_eq!(split("echo 'say \"hi\"'"), ["echo", "say \"hi\""]);
    }

    #[test]
    fn pieces_that_touch_are_one_word() {
        assert_eq!(split("a\"b c\"d"), ["ab cd"]);
        assert_eq!(split("'a'\"b\"c"), ["abc"]);
        assert_eq!(split("docs/'my file'"), ["docs/my file"]);
    }

    #[test]
    fn empty_quotes_are_a_word() {
        assert_eq!(split("given \"\" x"), ["given", "", "x"]);
        assert_eq!(split("''"), [""]);
        assert_eq!(split("a '' ''"), ["a", "", ""]);
        // Next to something, they add nothing.
        assert_eq!(split("a''b"), ["ab"]);
    }

    #[test]
    fn a_backslash_makes_the_next_character_ordinary() {
        assert_eq!(split("cd my\\ docs"), ["cd", "my docs"]);
        assert_eq!(split("echo \\\"quoted\\\""), ["echo", "\"quoted\""]);
        assert_eq!(split("echo a\\\\b"), ["echo", "a\\b"]);
        assert_eq!(split("echo \"a \\\" b\""), ["echo", "a \" b"]);
        assert_eq!(split("echo \"a\\\\b\""), ["echo", "a\\b"]);
        // An ordinary character stays what it was.
        assert_eq!(split("a\\b"), ["ab"]);
        // A backslash and a space, alone: a word that is one space.
        assert_eq!(split("\\ "), [" "]);
    }

    #[test]
    fn single_quotes_have_no_exceptions() {
        assert_eq!(split("'a\\b'"), ["a\\b"]);
        assert_eq!(split("'a\\'"), ["a\\"]);
    }

    #[test]
    fn what_is_never_closed_is_refused() {
        assert_eq!(words("echo \"unclosed"), Err(LineError::OpenQuote('"')));
        assert_eq!(words("echo 'unclosed"), Err(LineError::OpenQuote('\'')));
        assert_eq!(words("echo \"it's"), Err(LineError::OpenQuote('"')));
        assert_eq!(words("echo it's"), Err(LineError::OpenQuote('\'')));
        assert_eq!(words("echo \"a\\\""), Err(LineError::OpenQuote('"')));
        assert_eq!(words("echo a\\"), Err(LineError::TrailingBackslash));
        assert_eq!(words("echo \"a\\"), Err(LineError::TrailingBackslash));
    }

    #[test]
    fn letters_beyond_ascii_are_letters() {
        assert_eq!(split("write 'café au lait' ☕"), ["write", "café au lait", "☕"]);
    }

    #[test]
    fn the_errors_say_which_quote() {
        assert!(LineError::OpenQuote('\'').describe().contains('\''));
        assert!(LineError::OpenQuote('"').describe().contains('"'));
        assert!(LineError::TrailingBackslash.describe().contains('\\'));
    }
}
