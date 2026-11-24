//! A command's words, as sh splits them - for `system` (spawn.rs),
//! which starts a simple command itself, and hands anything more to sh
//! (sh.rs). No system in it: it is tested on the host, alone (`rustc
//! --test libc/src/words.rs`, in `make libc-test`).

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Why a command needs a shell to run.
#[derive(Debug, PartialEq, Eq)]
pub enum Unshelled {
    /// A character only a shell gives meaning to, outside quotes.
    Needs(char),
    /// A quote that is not closed, or a backslash at the end.
    Unfinished,
    /// `NAME=value command`: a variable for the command.
    Assignment,
    /// A pipe with no command on one side of it, or a redirection with
    /// no file after it.
    Malformed,
    /// A command that is a shell's own (spawn.rs says which): `echo`,
    /// `cd`, `exit`.
    Builtin(String),
}

/// Where one of a command's descriptors goes instead, said after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redirection {
    /// `< path`, `N< path`: read from the file (0 if no number).
    In { fd: u32, path: String },
    /// `> path`, `>> path`, `N> path`: written to the file, from empty
    /// or (`>>`) at its end (1 if no number).
    Out { fd: u32, path: String, append: bool },
    /// `N>&M`: `fd` is what `to` is at that point (`2>&1`; 1 if no N).
    Copy { fd: u32, to: u32 },
}

/// One command of a pipeline: its words, and its redirections in the
/// order they were said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stage {
    pub words: Vec<String>,
    pub redirections: Vec<Redirection>,
}

/// The words of `command` as sh would split it - quotes and backslashes
/// as sh takes them - if nothing in it needs a shell to mean what it
/// says: no pipes or redirections, no `;`, `&&` or `&`, no `$` or
/// backquotes, no wildcards, no `(`, `#` or `~` starting a word, no
/// variables set for the command. Those are for a shell: a command half
/// understood is worse than one handed on.
pub fn words_of(command: &str) -> Result<Vec<String>, Unshelled> {
    Ok(split(command, false)?.pop().map(|stage| stage.words).unwrap_or_default())
}

/// As `words_of`, but pipes and redirections are understood: the
/// commands of a pipeline, `a | b > out 2>&1`, each with its words and
/// redirections. Still refused: `;`, `&&`, `&`, `$`, backquotes,
/// wildcards, and the rest `words_of` refuses. An empty command is no
/// stages.
pub fn pipeline_of(command: &str) -> Result<Vec<Stage>, Unshelled> {
    split(command, true)
}

/// A redirection said, waiting for its file.
#[derive(Clone, Copy)]
enum Pending {
    In(u32),
    Out(u32, bool),
}

/// Both: with `operators` false, every `|`, `<` and `>` is refused as
/// needing a shell, and there is one stage.
fn split(command: &str, operators: bool) -> Result<Vec<Stage>, Unshelled> {
    let mut stages = Vec::new();
    let mut stage = Stage::default();
    let mut word = String::new();
    let mut in_word = false;
    // Whether any of the word was quoted or escaped: "2">f is not 2>f.
    let mut quoted = false;
    let mut pending: Option<Pending> = None;
    let mut chars = command.chars().peekable();
    // A finished word goes where it belongs: a redirection's file, or
    // the command's next word.
    let finish = |stage: &mut Stage, word: &mut String, in_word: &mut bool, quoted: &mut bool, pending: &mut Option<Pending>| {
        if !*in_word {
            return;
        }
        let word = core::mem::take(word);
        match pending.take() {
            Some(Pending::In(fd)) => stage.redirections.push(Redirection::In { fd, path: word }),
            Some(Pending::Out(fd, append)) => stage.redirections.push(Redirection::Out { fd, path: word, append }),
            None => stage.words.push(word),
        }
        *in_word = false;
        *quoted = false;
    };
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => finish(&mut stage, &mut word, &mut in_word, &mut quoted, &mut pending),
            '\'' => {
                in_word = true;
                quoted = true;
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
                quoted = true;
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
                    quoted = true;
                    word.push(c);
                }
                None => return Err(Unshelled::Unfinished),
            },
            '|' if operators => {
                if chars.peek() == Some(&'|') {
                    return Err(Unshelled::Needs('|')); // ||
                }
                finish(&mut stage, &mut word, &mut in_word, &mut quoted, &mut pending);
                if pending.is_some() || stage.words.is_empty() {
                    return Err(Unshelled::Malformed);
                }
                stages.push(core::mem::take(&mut stage));
            }
            '<' | '>' if operators => {
                // A number just before, unquoted, is the descriptor.
                let said = (in_word && !quoted && !word.is_empty() && word.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| word.parse::<u32>().ok())
                    .flatten();
                if said.is_some() {
                    word.clear();
                    in_word = false;
                } else {
                    finish(&mut stage, &mut word, &mut in_word, &mut quoted, &mut pending);
                }
                if pending.is_some() {
                    return Err(Unshelled::Malformed); // > > f
                }
                if c == '<' {
                    if matches!(chars.peek(), Some('<' | '&' | '>')) {
                        return Err(Unshelled::Needs('<')); // here-documents and the rest
                    }
                    pending = Some(Pending::In(said.unwrap_or(0)));
                    continue;
                }
                let fd = said.unwrap_or(1);
                match chars.peek() {
                    Some('>') => {
                        chars.next();
                        pending = Some(Pending::Out(fd, true));
                    }
                    Some('&') => {
                        chars.next();
                        let mut to = String::new();
                        while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                            to.push(d);
                            chars.next();
                        }
                        let ends = chars.peek().is_none_or(|c| matches!(c, ' ' | '\t' | '\n' | '|' | '<' | '>'));
                        match to.parse::<u32>() {
                            Ok(to) if ends => stage.redirections.push(Redirection::Copy { fd, to }),
                            _ => return Err(Unshelled::Needs('&')), // >&- and >&file
                        }
                    }
                    Some('|') => return Err(Unshelled::Needs('>')), // >|
                    _ => pending = Some(Pending::Out(fd, false)),
                }
            }
            '|' | '&' | ';' | '<' | '>' | '(' | ')' | '$' | '`' | '*' | '?' | '[' => return Err(Unshelled::Needs(c)),
            '#' | '~' if !in_word => return Err(Unshelled::Needs(c)),
            '=' if stage.words.is_empty() && pending.is_none() && !word.contains('/') => return Err(Unshelled::Assignment),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    finish(&mut stage, &mut word, &mut in_word, &mut quoted, &mut pending);
    if pending.is_some() {
        return Err(Unshelled::Malformed);
    }
    if stage.words.is_empty() {
        // Nothing at all is no stages; something with no command, or a
        // pipe with nothing after it, is a mistake.
        return if stages.is_empty() && stage.redirections.is_empty() { Ok(stages) } else { Err(Unshelled::Malformed) };
    }
    stages.push(stage);
    Ok(stages)
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

    fn stage(words: &[&str], redirections: Vec<Redirection>) -> Stage {
        Stage { words: words.iter().map(|w| String::from(*w)).collect(), redirections }
    }

    fn out(fd: u32, path: &str, append: bool) -> Redirection {
        Redirection::Out { fd, path: String::from(path), append }
    }

    #[test]
    fn pipes_and_redirections_are_understood() {
        assert_eq!(pipeline_of("cc -c x.c"), Ok(vec![stage(&["cc", "-c", "x.c"], vec![])]));
        assert_eq!(pipeline_of("  "), Ok(vec![]));
        assert_eq!(pipeline_of("ls|wc -l"), Ok(vec![stage(&["ls"], vec![]), stage(&["wc", "-l"], vec![])]));
        assert_eq!(pipeline_of("a | b | c").map(|s| s.len()), Ok(3));
        assert_eq!(pipeline_of("echo hi > out"), Ok(vec![stage(&["echo", "hi"], vec![out(1, "out", false)])]));
        assert_eq!(pipeline_of("echo hi >>log"), Ok(vec![stage(&["echo", "hi"], vec![out(1, "log", true)])]));
        assert_eq!(pipeline_of("sort < in > out"),
            Ok(vec![stage(&["sort"], vec![Redirection::In { fd: 0, path: String::from("in") }, out(1, "out", false)])]));
        assert_eq!(pipeline_of("cc x.c 2>errors"), Ok(vec![stage(&["cc", "x.c"], vec![out(2, "errors", false)])]));
        assert_eq!(pipeline_of("cc x.c >all 2>&1"),
            Ok(vec![stage(&["cc", "x.c"], vec![out(1, "all", false), Redirection::Copy { fd: 2, to: 1 }])]));
        assert_eq!(pipeline_of("echo oops >&2"), Ok(vec![stage(&["echo", "oops"], vec![Redirection::Copy { fd: 1, to: 2 }])]));
        assert_eq!(pipeline_of("> f echo x"), Ok(vec![stage(&["echo", "x"], vec![out(1, "f", false)])]), "anywhere in the command");
        assert_eq!(pipeline_of("echo 2 > f"), Ok(vec![stage(&["echo", "2"], vec![out(1, "f", false)])]), "a number apart is a word");
        assert_eq!(pipeline_of("echo \"2\">f"), Ok(vec![stage(&["echo", "2"], vec![out(1, "f", false)])]), "a quoted number too");
        assert_eq!(pipeline_of("echo x>'my file'"), Ok(vec![stage(&["echo", "x"], vec![out(1, "my file", false)])]));
        assert_eq!(pipeline_of("echo '|' \">\""), Ok(vec![stage(&["echo", "|", ">"], vec![])]), "quoted, they are words");
        assert_eq!(pipeline_of("CC=x"), Err(Unshelled::Assignment));
        assert_eq!(pipeline_of("cc -o x=y > a=b"), Ok(vec![stage(&["cc", "-o", "x=y"], vec![out(1, "a=b", false)])]));
    }

    #[test]
    fn what_still_needs_a_shell_is_refused() {
        for (command, c) in [("a && b", '&'), ("a || b", '|'), ("a; b", ';'), ("a &", '&'), ("a >&f", '&'), ("a 2>&-", '&'),
            ("a << END", '<'), ("a <> f", '<'), ("a >| f", '>'), ("echo $x > f", '$'), ("rm *.o", '*'), ("(a) | b", '(')]
        {
            assert_eq!(pipeline_of(command), Err(Unshelled::Needs(c)), "{command}");
        }
        for command in ["| a", "a |", "a | | b", "a >", "a > | b", "a < > f", "> f", "a > > f"] {
            assert_eq!(pipeline_of(command), Err(Unshelled::Malformed), "{command}");
        }
    }
}
