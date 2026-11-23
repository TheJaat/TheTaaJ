//! A command's words, as sh splits them - for `system` (spawn.rs),
//! which has no shell to hand the command to. No system in it: it is
//! tested on the host, alone (`rustc --test libc/src/words.rs`, in
//! `make libc-test`).

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Why a command cannot be run without a shell.
#[derive(Debug, PartialEq, Eq)]
pub enum Unshelled {
    /// A character only a shell gives meaning to, outside quotes.
    Needs(char),
    /// A quote that is not closed, or a backslash at the end.
    Unfinished,
    /// `NAME=value command`: a variable for the command.
    Assignment,
}

/// The words of `command` as sh would split it - quotes and backslashes
/// as sh takes them - if nothing in it needs a shell to mean what it
/// says: no pipes or redirections, no `;`, `&&` or `&`, no `$` or
/// backquotes, no wildcards, no `(`, `#` or `~` starting a word, no
/// variables set for the command. There is no shell for a C program on
/// TheTaaJ, and a command half understood is worse than one refused.
pub fn words_of(command: &str) -> Result<Vec<String>, Unshelled> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => {
                if in_word {
                    words.push(core::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err(Unshelled::Unfinished),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => word.push(c),
                            Some('\n') => {}
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err(Unshelled::Unfinished),
                        },
                        Some(c @ ('$' | '`')) => return Err(Unshelled::Needs(c)),
                        Some(c) => word.push(c),
                        None => return Err(Unshelled::Unfinished),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(c) => {
                    in_word = true;
                    word.push(c);
                }
                None => return Err(Unshelled::Unfinished),
            },
            '|' | '&' | ';' | '<' | '>' | '(' | ')' | '$' | '`' | '*' | '?' | '[' => return Err(Unshelled::Needs(c)),
            '#' | '~' if !in_word => return Err(Unshelled::Needs(c)),
            '=' if words.is_empty() && !word.contains('/') => return Err(Unshelled::Assignment),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(command: &str) -> Result<Vec<&str>, Unshelled> {
        // Leaked: a test's strings, for comparing as &str.
        words_of(command).map(|words| words.into_iter().map(|w| &*Box::leak(w.into_boxed_str())).collect())
    }

    #[test]
    fn a_command_is_split_as_sh_splits_it() {
        assert_eq!(words("clang  -c x.c\t-o x.o"), Ok(vec!["clang", "-c", "x.c", "-o", "x.o"]));
        assert_eq!(words("echo 'a b' \"c d\" e\\ f ''"), Ok(vec!["echo", "a b", "c d", "e f", ""]));
        assert_eq!(words("echo \"say \\\"hi\\\"\" 'it''s' a\\\nb"), Ok(vec!["echo", "say \"hi\"", "its", "ab"]));
        assert_eq!(words("cc -DX=1 -o a=b"), Ok(vec!["cc", "-DX=1", "-o", "a=b"]));
        assert_eq!(words("dir/x=y"), Ok(vec!["dir/x=y"]));
        assert_eq!(words("  "), Ok(vec![]));
        assert_eq!(words("echo '*' \"|;&\" \\$x"), Ok(vec!["echo", "*", "|;&", "$x"]));
        assert_eq!(words("echo a#b x~"), Ok(vec!["echo", "a#b", "x~"]));
    }

    #[test]
    fn what_needs_a_shell_is_refused() {
        for (command, c) in [("a | b", '|'), ("a && b", '&'), ("a; b", ';'), ("a > f", '>'), ("a < f", '<'), ("echo $HOME", '$'),
            ("echo `x`", '`'), ("rm *.o", '*'), ("ls ?", '?'), ("ls [ab]", '['), ("(cd x)", '('), ("echo # no", '#'),
            ("cd ~", '~'), ("echo \"$x\"", '$')]
        {
            assert_eq!(words(command), Err(Unshelled::Needs(c)), "{command}");
        }
        assert_eq!(words("CC=gcc make"), Err(Unshelled::Assignment));
        assert_eq!(words("echo 'open"), Err(Unshelled::Unfinished));
        assert_eq!(words("echo \"open"), Err(Unshelled::Unfinished));
        assert_eq!(words("echo end\\"), Err(Unshelled::Unfinished));
    }
}
