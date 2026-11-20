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
//! Three more things are understood, when they are not quoted and not
//! after a backslash - where the command's output goes, and where its
//! input comes from:
//!
//!   - `> FILE`: what the command prints goes into FILE, replacing
//!     what it held;
//!   - `>> FILE`: the same, added to the end of FILE;
//!   - `< FILE`: what the command reads comes from FILE.
//!
//! They may stand anywhere on the line and need no spaces around them:
//! `ls>list` is `ls > list`. They are not arguments - the command never
//! sees them. In quotes they are ordinary characters: `echo "a > b"`
//! prints `a > b`. That is why this cannot be done on finished words:
//! once the quotes are gone, a `>` that was typed as text and a `>`
//! that was typed as an instruction look the same.
//!
//! And one that joins commands: `A | B` is two commands, and what A
//! prints is what B reads - a pipeline, of any number of them. Only
//! the first can take its input from a file and only the last can send
//! its output to one: everywhere else that end is already joined to a
//! neighbour. (A Unix shell lets the file win; that is nearly always a
//! mistake when typed, so here it is refused.) A quoted `|` is text.
//!
//! And one that says how to run them: a `&` at the very end of the line
//! means "in the background" - start it, and do not wait for it.
//! Anywhere else an unquoted `&` is refused: `a & b` and `a && b` mean
//! things in a Unix shell that this one does not do, and guessing would
//! run something the user did not ask for.
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
    /// A `>`, `>>` or `<` with no file name after it.
    NoFile(Redirect),
    /// Two places for the output, or two for the input.
    Twice(Redirect),
    /// A `|` with no command on one side of it.
    NoCommand,
    /// In a pipeline: an input file for a command that is not the
    /// first, or an output file for one that is not the last.
    Misplaced(Redirect),
    /// A `&` that is not the last thing on the line.
    NotLast,
    /// A `&` with no command before it to run.
    NothingToRun,
}

/// Which way a redirection points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redirect {
    /// `>`
    Out,
    /// `>>`
    Append,
    /// `<`
    In,
}

/// One command of a line, understood: the command and its arguments,
/// and where its output and input are to go if not the usual places.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Command {
    pub words: Vec<String>,
    /// The file the output goes to, and whether it is added to the end
    /// of it (`>>`) rather than replacing it (`>`).
    pub output: Option<(String, bool)>,
    /// The file the input comes from.
    pub input: Option<String>,
}

/// A whole line, understood.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    /// Its commands, in order: one, or several joined by `|`. There is
    /// always at least one - for a blank line, one with no words.
    pub commands: Vec<Command>,
    /// Whether the line ended in `&`: run it, and do not wait for it.
    pub background: bool,
}

/// A piece of a line: a word, or an instruction about the words.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    To(Redirect),
    /// `|`
    Pipe,
    /// `&`
    Background,
}

impl LineError {
    pub fn describe(self) -> &'static str {
        match self {
            LineError::OpenQuote('\'') => "the line has a ' that is never closed",
            LineError::OpenQuote(_) => "the line has a \" that is never closed",
            LineError::TrailingBackslash => "the line ends in a \\ with nothing after it",
            LineError::NoFile(Redirect::Out) => "there is a > with no file name after it",
            LineError::NoFile(Redirect::Append) => "there is a >> with no file name after it",
            LineError::NoFile(Redirect::In) => "there is a < with no file name after it",
            LineError::Twice(Redirect::In) => "the input can come from only one file",
            LineError::Twice(_) => "the output can go to only one file",
            LineError::NoCommand => "there is a | with no command on one side of it",
            LineError::Misplaced(Redirect::In) => "only the first command of a pipeline can take its input from a file",
            LineError::Misplaced(_) => "only the last command of a pipeline can send its output to a file",
            LineError::NotLast => "a & can only end the line",
            LineError::NothingToRun => "there is a & with no command before it",
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

/// The line, understood: its commands, and whether to wait for them.
pub fn parse(line: &str) -> Result<Line, LineError> {
    let mut commands = Vec::new();
    let mut background = false;
    // The command being collected; a `|` finishes it and begins the
    // next.
    let mut command = Command::default();
    let mut tokens = tokens(line)?.into_iter();
    while let Some(token) = tokens.next() {
        // Whatever this token is, it stands after the `&`.
        if background {
            return Err(LineError::NotLast);
        }
        let redirect = match token {
            Token::Word(word) => {
                command.words.push(word);
                continue;
            }
            Token::Pipe => {
                // `take` hands over the finished command and leaves an
                // empty one in its place.
                commands.push(core::mem::take(&mut command));
                continue;
            }
            Token::Background => {
                background = true;
                continue;
            }
            Token::To(redirect) => redirect,
        };
        // The token after a redirection is its file - and must be a
        // word: `ls > > x` and `ls >` name none.
        let Some(Token::Word(file)) = tokens.next() else {
            return Err(LineError::NoFile(redirect));
        };
        // `replace` puts the new value in and hands back the old one:
        // if there was one, this is the second time.
        let before = match redirect {
            Redirect::In => command.input.replace(file).is_some(),
            _ => command.output.replace((file, redirect == Redirect::Append)).is_some(),
        };
        if before {
            return Err(LineError::Twice(redirect));
        }
    }
    commands.push(command);
    if background && commands.iter().any(|command| command.words.is_empty()) {
        return Err(LineError::NothingToRun);
    }
    if commands.len() > 1 {
        // `a |`, `| b`, `a | | b`: a side with nothing to run.
        if commands.iter().any(|command| command.words.is_empty()) {
            return Err(LineError::NoCommand);
        }
        let last = commands.len() - 1;
        for (n, command) in commands.iter().enumerate() {
            if n > 0 && command.input.is_some() {
                return Err(LineError::Misplaced(Redirect::In));
            }
            if n < last && command.output.is_some() {
                return Err(LineError::Misplaced(Redirect::Out));
            }
        }
    }
    Ok(Line { commands, background })
}

/// The pieces of `line`, in order.
fn tokens(line: &str) -> Result<Vec<Token>, LineError> {
    let mut tokens = Vec::new();
    // The word being collected - and whether there is one at all: `""`
    // is a word though it has no characters, so the text alone cannot
    // say. `Option` carries that: None is "between words".
    let mut word: Option<String> = None;
    let mut within = Within::Nothing;
    // `peekable` lets the loop look at the next character without
    // taking it: `>` and `>>` differ only in what follows.
    let mut characters = line.chars().peekable();

    // `while let` takes characters until there are none. The loop body
    // may take one more itself - the character after a backslash -
    // which a `for` loop, holding the iterator for itself, would not
    // allow.
    while let Some(c) = characters.next() {
        match (within, c) {
            // Between words: space ends the word, if one was begun.
            (Within::Nothing, ' ' | '\t') => tokens.extend(word.take().map(Token::Word)),
            // A redirection ends the word before it, as a space would.
            (Within::Nothing, '>' | '<') => {
                tokens.extend(word.take().map(Token::Word));
                let redirect = match c {
                    '<' => Redirect::In,
                    // `next_if_eq` takes the next character only if it
                    // is the one named.
                    _ if characters.next_if_eq(&'>').is_some() => Redirect::Append,
                    _ => Redirect::Out,
                };
                tokens.push(Token::To(redirect));
            }
            // And so do a pipe, and the sign for the background.
            (Within::Nothing, '|' | '&') => {
                tokens.extend(word.take().map(Token::Word));
                tokens.push(if c == '|' { Token::Pipe } else { Token::Background });
            }
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
            tokens.extend(word.map(Token::Word));
            Ok(tokens)
        }
        Within::Single => Err(LineError::OpenQuote('\'')),
        Within::Double => Err(LineError::OpenQuote('"')),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The line's commands - for the tests that are not about `&`.
    /// (It hides the `parse` of the module for the rest of the tests.)
    fn parse(line: &str) -> Result<Vec<Command>, LineError> {
        super::parse(line).map(|line| line.commands)
    }

    /// The line as one command - which the lines of most tests are.
    fn one(line: &str) -> Result<Command, LineError> {
        let mut commands = parse(line)?;
        assert_eq!(commands.len(), 1, "{line:?} is more than one command");
        Ok(commands.remove(0))
    }

    fn words(line: &str) -> Result<Vec<String>, LineError> {
        one(line).map(|command| command.words)
    }

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

    fn command(words: &[&str], output: Option<(&str, bool)>, input: Option<&str>) -> Command {
        Command {
            words: words.iter().map(|w| w.to_string()).collect(),
            output: output.map(|(file, append)| (file.to_string(), append)),
            input: input.map(str::to_string),
        }
    }

    #[test]
    fn output_and_input_are_named_after_their_signs() {
        assert_eq!(one("ls > list"), Ok(command(&["ls"], Some(("list", false)), None)));
        assert_eq!(one("echo more >> list"), Ok(command(&["echo", "more"], Some(("list", true)), None)));
        assert_eq!(one("wc < list"), Ok(command(&["wc"], None, Some("list"))));
        assert_eq!(one("wc < in > out"), Ok(command(&["wc"], Some(("out", false)), Some("in"))));
        assert_eq!(one("ls docs"), Ok(command(&["ls", "docs"], None, None)));
    }

    #[test]
    fn redirections_need_no_spaces_and_may_stand_anywhere() {
        assert_eq!(one("ls>list"), Ok(command(&["ls"], Some(("list", false)), None)));
        assert_eq!(one("echo a>>b c"), Ok(command(&["echo", "a", "c"], Some(("b", true)), None)));
        assert_eq!(one("> out echo hi"), Ok(command(&["echo", "hi"], Some(("out", false)), None)));
        assert_eq!(one("wc<in>out"), Ok(command(&["wc"], Some(("out", false)), Some("in"))));
        assert_eq!(one("cat >'my file'"), Ok(command(&["cat"], Some(("my file", false)), None)));
        // Only the sign, with nothing run: the words are none.
        assert_eq!(one("> empty"), Ok(command(&[], Some(("empty", false)), None)));
    }

    #[test]
    fn a_quoted_sign_is_text() {
        assert_eq!(one("echo \"a > b\""), Ok(command(&["echo", "a > b"], None, None)));
        assert_eq!(one("echo '<' \\> x\\>\\>y"), Ok(command(&["echo", "<", ">", "x>>y"], None, None)));
        assert_eq!(one("echo a\">\"b > c"), Ok(command(&["echo", "a>b"], Some(("c", false)), None)));
        // And a file may be named by one.
        assert_eq!(one("echo x > '>'"), Ok(command(&["echo", "x"], Some((">", false)), None)));
    }

    #[test]
    fn a_redirection_to_nowhere_or_to_two_places_is_refused() {
        assert_eq!(one("ls >"), Err(LineError::NoFile(Redirect::Out)));
        assert_eq!(one("ls >>"), Err(LineError::NoFile(Redirect::Append)));
        assert_eq!(one("wc <"), Err(LineError::NoFile(Redirect::In)));
        assert_eq!(one("ls > > x"), Err(LineError::NoFile(Redirect::Out)));
        assert_eq!(one("ls >>> x"), Err(LineError::NoFile(Redirect::Append)));
        assert_eq!(one("ls > < x"), Err(LineError::NoFile(Redirect::Out)));
        assert_eq!(one("ls > a > b"), Err(LineError::Twice(Redirect::Out)));
        assert_eq!(one("ls > a >> b"), Err(LineError::Twice(Redirect::Append)));
        assert_eq!(one("wc < a < b"), Err(LineError::Twice(Redirect::In)));
        // An empty name is a name - a bad one, for the file service to refuse.
        assert_eq!(one("ls > ''"), Ok(command(&["ls"], Some(("", false)), None)));
    }

    #[test]
    fn a_pipe_joins_commands() {
        assert_eq!(parse("ls | wc"), Ok(vec![command(&["ls"], None, None), command(&["wc"], None, None)]));
        // No spaces needed, and any number of commands.
        assert_eq!(
            parse("cat a b|cat|wc"),
            Ok(vec![command(&["cat", "a", "b"], None, None), command(&["cat"], None, None), command(&["wc"], None, None)])
        );
        // One command is a pipeline of one; a blank line, of one with no words.
        assert_eq!(parse("ls").map(|commands| commands.len()), Ok(1));
        assert_eq!(parse("  "), Ok(vec![Command::default()]));
    }

    #[test]
    fn a_pipeline_reads_a_file_at_its_start_and_writes_one_at_its_end() {
        assert_eq!(
            parse("cat < in | wc > out"),
            Ok(vec![command(&["cat"], None, Some("in")), command(&["wc"], Some(("out", false)), None)])
        );
        assert_eq!(parse("a | b >> out").map(|commands| commands[1].output.clone()), Ok(Some(("out".to_string(), true))));
        assert_eq!(parse("a > x | b"), Err(LineError::Misplaced(Redirect::Out)));
        assert_eq!(parse("a | b > x | c"), Err(LineError::Misplaced(Redirect::Out)));
        assert_eq!(parse("a | b < x"), Err(LineError::Misplaced(Redirect::In)));
        // Each command has its own: two outputs are "twice" only within one.
        assert_eq!(parse("a | b > x > y"), Err(LineError::Twice(Redirect::Out)));
    }

    #[test]
    fn a_pipe_needs_a_command_on_both_sides() {
        assert_eq!(parse("ls |"), Err(LineError::NoCommand));
        assert_eq!(parse("| wc"), Err(LineError::NoCommand));
        assert_eq!(parse("ls | | wc"), Err(LineError::NoCommand));
        assert_eq!(parse("ls || wc"), Err(LineError::NoCommand));
        assert_eq!(parse("|"), Err(LineError::NoCommand));
        // A redirection alone is not a command.
        assert_eq!(parse("ls | > out"), Err(LineError::NoCommand));
        // The file a redirection names cannot be a pipe.
        assert_eq!(parse("ls > | wc"), Err(LineError::NoFile(Redirect::Out)));
    }

    #[test]
    fn a_quoted_pipe_is_text() {
        assert_eq!(one("echo \"p | q\""), Ok(command(&["echo", "p | q"], None, None)));
        assert_eq!(one("echo a\\|b '|'"), Ok(command(&["echo", "a|b", "|"], None, None)));
    }

    #[test]
    fn a_last_ampersand_means_the_background() {
        let line = super::parse("forever sleep &").unwrap();
        assert!(line.background);
        assert_eq!(line.commands, [command(&["forever", "sleep"], None, None)]);
        // No space needed; and it is not an argument.
        assert_eq!(super::parse("yes&").unwrap().commands, [command(&["yes"], None, None)]);
        // The whole pipeline, with its files.
        let line = super::parse("cat < in | wc > out &").unwrap();
        assert!(line.background);
        assert_eq!(line.commands.len(), 2);
        assert_eq!(line.commands[1].output, Some(("out".to_string(), false)));
        // And without one, not.
        assert!(!super::parse("yes").unwrap().background);
    }

    #[test]
    fn an_ampersand_anywhere_else_is_refused() {
        assert_eq!(parse("a & b"), Err(LineError::NotLast));
        assert_eq!(parse("a && b"), Err(LineError::NotLast));
        assert_eq!(parse("a & &"), Err(LineError::NotLast));
        assert_eq!(parse("a & > f"), Err(LineError::NotLast));
        assert_eq!(parse("a & | b"), Err(LineError::NotLast));
        assert_eq!(parse("&"), Err(LineError::NothingToRun));
        assert_eq!(parse("  & "), Err(LineError::NothingToRun));
        assert_eq!(parse("> f &"), Err(LineError::NothingToRun));
        // The file a redirection names cannot be one.
        assert_eq!(parse("ls > &"), Err(LineError::NoFile(Redirect::Out)));
    }

    #[test]
    fn a_quoted_ampersand_is_text() {
        assert_eq!(one("echo \"a & b\" '&' \\&"), Ok(command(&["echo", "a & b", "&", "&"], None, None)));
        assert!(!super::parse("echo 'x &'").unwrap().background);
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
