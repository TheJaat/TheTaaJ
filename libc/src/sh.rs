//! A small POSIX shell's language - what `sh -c`, `system` and make's
//! recipes need of one:
//!   - lists (`;`, `&&`, `||`), pipelines (`|`, `!`), redirections;
//!   - `if`, `while`, `until`, `for`, `case`, `{ ...; }`, `( ... )`,
//!     functions, `break`, `continue`, `return`;
//!   - variables (`NAME=value`, `$NAME`, `${NAME:-word}`, `${NAME%.c}`,
//!     `"$@"`, `$?`), `$((...))`, command substitution (`$(...)` and
//!     backquotes), quoting, wildcards;
//!   - the builtins a recipe uses (cd, echo, test, read, export, set -e,
//!     eval and the rest).
//!
//! Not yet: here-documents, `&`, `<>`. Those are refused, by name,
//! before anything runs - a script half understood is worse than one
//! refused.
//!
//! No system in it: what it asks of one - start a pipeline, read a
//! directory for a wildcard, change directory - is the `System` trait,
//! which `sh_system.rs` gives it on TheTaaJ. So it is tested on the
//! host, alone (`rustc --test libc/src/sh.rs`, in `make libc-test`),
//! against a pretend system.

extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

// ---- what the shell asks of a system ---------------------------------

/// Where one of a command's descriptors goes, as the system is to do
/// it, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redirection {
    /// `< path`: read from the file.
    In { fd: u32, path: String },
    /// `> path`, `>> path` (`append`).
    Out { fd: u32, path: String, append: bool },
    /// `N>&M`, `N<&M`: `fd` a copy of `to`.
    Copy { fd: u32, to: u32 },
    /// `N>&-`: closed.
    Close { fd: u32 },
}

/// A script for another copy of this shell to run, as a Unix shell's
/// fork would: `sh -c 'FUNCTIONS SCRIPT' ZERO WORDS...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copy {
    /// The definitions of this shell's functions, so that the copy has
    /// them too; empty if there are none.
    pub functions: String,
    pub script: String,
    /// Its `$0`.
    pub zero: String,
}

/// One command of a pipeline, its words expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// A program and its arguments; for a copy of the shell, its `$1`
    /// on.
    pub words: Vec<String>,
    pub redirections: Vec<Redirection>,
    /// `NAME=value` strings: what the program's environment is.
    pub environment: Vec<String>,
    /// Run by another copy of this shell, not as a program: a builtin
    /// or a function in a pipeline or with redirections (its script is
    /// `"$@"`), or a compound command (`for ...; done > list`).
    pub shell: Option<Copy>,
}

/// What a path is, for `test`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File { size: u64 },
    Directory,
}

/// What the shell needs done by the system it runs on.
pub trait System {
    /// Starts the commands, each one's output the next one's input,
    /// and waits for them: the last one's status, as `$?` has it (0 to
    /// 255; 128 + N for a program stopped by signal N; 127 for one not
    /// found, said on standard error).
    fn pipeline(&mut self, commands: &[Command]) -> i32;
    /// Runs a command - a copy of this shell - and gives what it wrote
    /// to standard output, and its status.
    fn capture(&mut self, command: &Command) -> (String, i32);
    /// The paths that match `pattern` (a backslash makes the next
    /// character itself), sorted; none if nothing does.
    fn glob(&mut self, pattern: &str) -> Vec<String>;
    fn chdir(&mut self, path: &str) -> Result<(), String>;
    /// The current directory, from the root.
    fn cwd(&mut self) -> String;
    fn kind(&mut self, path: &str) -> Option<Kind>;
    /// Writes to one of this shell's descriptors (1 or 2).
    fn write(&mut self, fd: u32, text: &str);
    /// A line of standard input, without its new line; None at the end.
    fn read_line(&mut self) -> Option<String>;
    fn pid(&self) -> i32;
}

// ---- the language: words ----------------------------------------------

/// A word as written, before it is expanded: its parts in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Word {
    parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    /// Characters as written, outside quotes: wildcards work in them.
    Plain(String),
    /// Quoted or escaped characters: themselves.
    Quoted(String),
    /// `$NAME`, `${NAME...}`; `quoted` inside double quotes.
    Parameter { name: String, op: Op, quoted: bool },
    /// `$(script)` or `` `script` ``: its output.
    Substitution { script: String, quoted: bool },
    /// `$((expression))`: its value.
    Arithmetic { expression: String, quoted: bool },
    /// `~` starting a word: $HOME.
    Home,
}

/// What `${NAME...}` does with NAME's value.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    Value,
    /// `${#NAME}`
    Length,
    /// `${NAME-word}`, `${NAME:-word}` (`colon`: empty counts as unset).
    Default { colon: bool, word: Word },
    /// `${NAME=word}`, `${NAME:=word}`: and NAME is set to it.
    Assign { colon: bool, word: Word },
    /// `${NAME+word}`, `${NAME:+word}`: word if NAME is set.
    Alternative { colon: bool, word: Word },
    /// `${NAME%pattern}`, `%%` (`longest`), and from the front (`prefix`)
    /// `${NAME#pattern}`, `##`: the value without what matches.
    Remove { prefix: bool, longest: bool, pattern: Word },
}

impl Word {
    /// The word's text, if it is only plain characters - for reserved
    /// words, names and descriptor numbers.
    fn plain(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [Part::Plain(text)] => Some(text),
            _ => None,
        }
    }
}

// ---- the language: what can go wrong -----------------------------------

/// Why a script cannot be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A mistake: what was found, where something else was expected.
    Syntax(String),
    /// Something sh has that this one does not have yet.
    Missing(&'static str),
}

impl Refused {
    pub fn describe(&self) -> String {
        match self {
            Refused::Syntax(what) => format!("syntax error: {what}"),
            Refused::Missing(what) => format!("{what} is not in this shell yet"),
        }
    }
}

// ---- reading a script into tokens ---------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(Word),
    /// Digits just before `<` or `>`: the descriptor redirected.
    Number(u32),
    Operator(&'static str),
}

/// The operators, longest first, so that `&&` is not read as `&`.
const OPERATORS: [&str; 17] = ["&&", "||", ";;", "<<", ">>", "<&", ">&", "<>", ">|", "&", "|", ";", "<", ">", "(", ")", "\n"];

struct Lexer {
    chars: Vec<char>,
    at: usize,
}

impl Lexer {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += 1;
        Some(c)
    }

    /// The tokens, each with where it starts and ends in the script.
    fn tokens(&mut self) -> Result<Vec<(Token, usize, usize)>, Refused> {
        let mut tokens = Vec::new();
        loop {
            let start = self.at;
            match self.peek() {
                None => return Ok(tokens),
                Some(' ' | '\t') => self.at += 1,
                Some('\\') if self.peek_at(1) == Some('\n') => self.at += 2,
                Some('#') => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.at += 1;
                    }
                }
                Some(_) => {
                    if let Some(op) = OPERATORS.iter().find(|op| self.chars[self.at..].starts_with(&op.chars().collect::<Vec<_>>())) {
                        self.at += op.chars().count();
                        tokens.push((Token::Operator(op), start, self.at));
                        continue;
                    }
                    let word = self.word()?;
                    let number = word.plain().filter(|text| text.bytes().all(|b| b.is_ascii_digit()));
                    let token = match number {
                        Some(digits) if matches!(self.peek(), Some('<' | '>')) => {
                            Token::Number(digits.parse().map_err(|_| Refused::Syntax(format!("descriptor {digits}")))?)
                        }
                        _ => Token::Word(word),
                    };
                    tokens.push((token, start, self.at));
                }
            }
        }
    }

    /// A word, up to a blank or an operator outside quotes.
    fn word(&mut self) -> Result<Word, Refused> {
        let mut parts = Vec::new();
        let start = self.at;
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\n' | '|' | '&' | ';' | '<' | '>' | '(' | ')' => break,
                '\'' => {
                    self.at += 1;
                    let text = self.single_quoted()?;
                    push_text(&mut parts, &text, true);
                }
                '"' => {
                    self.at += 1;
                    self.double_quoted(&mut parts)?;
                }
                '\\' => {
                    self.at += 1;
                    match self.next() {
                        Some('\n') => {}
                        Some(c) => push_text(&mut parts, c.encode_utf8(&mut [0; 4]), true),
                        None => return Err(Refused::Syntax(String::from("a \\ at the end"))),
                    }
                }
                '$' => {
                    self.at += 1;
                    let part = self.dollar(false)?;
                    parts.push(part);
                }
                '`' => {
                    self.at += 1;
                    let script = self.backquoted()?;
                    parts.push(Part::Substitution { script, quoted: false });
                }
                '~' if self.at == start && matches!(self.peek_at(1), None | Some('/' | ' ' | '\t' | '\n' | ';' | '&' | '|' | ')')) => {
                    self.at += 1;
                    parts.push(Part::Home);
                }
                c => {
                    self.at += 1;
                    push_text(&mut parts, c.encode_utf8(&mut [0; 4]), false);
                }
            }
        }
        Ok(Word { parts })
    }

    /// After a `'`: up to the closing one.
    fn single_quoted(&mut self) -> Result<String, Refused> {
        let mut text = String::new();
        loop {
            match self.next() {
                Some('\'') => return Ok(text),
                Some(c) => text.push(c),
                None => return Err(Refused::Syntax(String::from("a ' with no ' after it"))),
            }
        }
    }

    /// After a `"`: up to the closing one.
    fn double_quoted(&mut self, parts: &mut Vec<Part>) -> Result<(), Refused> {
        let before = parts.len();
        loop {
            match self.next() {
                None => return Err(Refused::Syntax(String::from("a \" with no \" after it"))),
                Some('"') => {
                    // An empty "" is still a word: an empty quoted part.
                    // ("$@" with no arguments is not: it is no word.)
                    if parts.len() == before && !matches!(parts.last(), Some(Part::Quoted(_))) {
                        parts.push(Part::Quoted(String::new()));
                    }
                    return Ok(());
                }
                Some('\\') => match self.next() {
                    Some('\n') => {}
                    Some(c @ ('$' | '`' | '"' | '\\')) => push_text(parts, c.encode_utf8(&mut [0; 4]), true),
                    Some(c) => {
                        push_text(parts, "\\", true);
                        push_text(parts, c.encode_utf8(&mut [0; 4]), true);
                    }
                    None => return Err(Refused::Syntax(String::from("a \" with no \" after it"))),
                },
                Some('$') => {
                    let part = self.dollar(true)?;
                    parts.push(part);
                }
                Some('`') => {
                    let script = self.backquoted()?;
                    parts.push(Part::Substitution { script, quoted: true });
                }
                Some(c) => push_text(parts, c.encode_utf8(&mut [0; 4]), true),
            }
        }
    }

    /// After a `$`.
    fn dollar(&mut self, quoted: bool) -> Result<Part, Refused> {
        match self.peek() {
            Some('{') => {
                self.at += 1;
                self.braced(quoted)
            }
            Some('(') if self.peek_at(1) == Some('(') => {
                self.at += 2;
                let expression = self.arithmetic()?;
                Ok(Part::Arithmetic { expression, quoted })
            }
            Some('(') => {
                self.at += 1;
                let script = self.parenthesised()?;
                Ok(Part::Substitution { script, quoted })
            }
            Some(c) if c == '_' || c.is_ascii_alphabetic() => {
                let name = self.name();
                Ok(Part::Parameter { name, op: Op::Value, quoted })
            }
            Some(c @ ('0'..='9' | '?' | '#' | '@' | '*' | '$' | '!' | '-')) => {
                self.at += 1;
                Ok(Part::Parameter { name: c.to_string(), op: Op::Value, quoted })
            }
            // A `$` before anything else is itself.
            _ => Ok(if quoted { Part::Quoted(String::from("$")) } else { Part::Plain(String::from("$")) }),
        }
    }

    fn name(&mut self) -> String {
        let mut name = String::new();
        while let Some(c) = self.peek().filter(|&c| c == '_' || c.is_ascii_alphanumeric()) {
            name.push(c);
            self.at += 1;
        }
        name
    }

    /// After `${`.
    fn braced(&mut self, quoted: bool) -> Result<Part, Refused> {
        let unclosed = || Refused::Syntax(String::from("a ${ with no } after it"));
        let length = self.peek() == Some('#') && self.peek_at(1).is_some_and(|c| c != '}');
        if length {
            self.at += 1;
        }
        let name = match self.peek() {
            Some(c) if c == '_' || c.is_ascii_alphabetic() => self.name(),
            Some('0'..='9') => {
                let mut digits = String::new();
                while let Some(d) = self.peek().filter(char::is_ascii_digit) {
                    digits.push(d);
                    self.at += 1;
                }
                digits
            }
            Some(c @ ('?' | '#' | '@' | '*' | '$' | '!' | '-')) => {
                self.at += 1;
                c.to_string()
            }
            _ => return Err(Refused::Syntax(String::from("a ${...} with no name in it"))),
        };
        if length {
            return match self.next() {
                Some('}') => Ok(Part::Parameter { name, op: Op::Length, quoted }),
                _ => Err(unclosed()),
            };
        }
        if let Some(c @ ('%' | '#')) = self.peek() {
            self.at += 1;
            let longest = self.peek() == Some(c);
            if longest {
                self.at += 1;
            }
            // A pattern is one inside double quotes too: "${f##*/}".
            let pattern = self.word_to_brace(false)?;
            return Ok(Part::Parameter { name, op: Op::Remove { prefix: c == '#', longest, pattern }, quoted });
        }
        let colon = self.peek() == Some(':');
        if colon {
            self.at += 1;
        }
        let kind = match self.next() {
            Some('}') if !colon => return Ok(Part::Parameter { name, op: Op::Value, quoted }),
            Some(c @ ('-' | '=' | '+')) => c,
            Some('?') => return Err(Refused::Missing("${NAME?message}")),
            None => return Err(unclosed()),
            Some(c) => return Err(Refused::Syntax(format!("'{c}' in ${{{name}...}}"))),
        };
        let word = self.word_to_brace(quoted)?;
        let op = match kind {
            '-' => Op::Default { colon, word },
            '=' => Op::Assign { colon, word },
            _ => Op::Alternative { colon, word },
        };
        Ok(Part::Parameter { name, op, quoted })
    }

    /// The word in `${NAME-word}`, up to the `}` that closes it.
    fn word_to_brace(&mut self, quoted: bool) -> Result<Word, Refused> {
        let mut parts = Vec::new();
        loop {
            match self.next() {
                None => return Err(Refused::Syntax(String::from("a ${ with no } after it"))),
                Some('}') => return Ok(Word { parts }),
                Some('\'') if !quoted => {
                    let text = self.single_quoted()?;
                    push_text(&mut parts, &text, true);
                }
                Some('"') if !quoted => self.double_quoted(&mut parts)?,
                Some('\\') => match self.next() {
                    Some(c) => push_text(&mut parts, c.encode_utf8(&mut [0; 4]), true),
                    None => return Err(Refused::Syntax(String::from("a \\ at the end"))),
                },
                Some('$') => {
                    let part = self.dollar(quoted)?;
                    parts.push(part);
                }
                Some(c) => push_text(&mut parts, c.encode_utf8(&mut [0; 4]), quoted),
            }
        }
    }

    /// After `$((`: the expression, up to the `))` that closes it.
    fn arithmetic(&mut self) -> Result<String, Refused> {
        let start = self.at;
        let mut depth = 0;
        loop {
            match self.next() {
                None => return Err(Refused::Syntax(String::from("a $(( with no )) after it"))),
                Some('(') => depth += 1,
                Some(')') if depth > 0 => depth -= 1,
                Some(')') if self.peek() == Some(')') => {
                    self.at += 1;
                    return Ok(self.chars[start..self.at - 2].iter().collect());
                }
                Some(')') => return Err(Refused::Syntax(String::from("a $(( closed by one ) only"))),
                Some(_) => {}
            }
        }
    }

    /// After `$(`: the script, up to the `)` that closes it.
    fn parenthesised(&mut self) -> Result<String, Refused> {
        let start = self.at;
        let mut depth = 1;
        loop {
            match self.next() {
                None => return Err(Refused::Syntax(String::from("a $( with no ) after it"))),
                Some('(') => depth += 1,
                Some(')') => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(self.chars[start..self.at - 1].iter().collect());
                    }
                }
                Some('\\') => self.at += 1,
                Some('\'') => {
                    while self.next().is_some_and(|c| c != '\'') {}
                }
                Some('"') => loop {
                    match self.next() {
                        None | Some('"') => break,
                        Some('\\') => self.at += 1,
                        Some(_) => {}
                    }
                },
                Some(_) => {}
            }
        }
    }

    /// After a backquote: the script, up to the next one; `\``, `\\`
    /// and `\$` there are the character.
    fn backquoted(&mut self) -> Result<String, Refused> {
        let mut script = String::new();
        loop {
            match self.next() {
                None => return Err(Refused::Syntax(String::from("a ` with no ` after it"))),
                Some('`') => return Ok(script),
                Some('\\') => match self.next() {
                    Some(c @ ('`' | '\\' | '$')) => script.push(c),
                    Some(c) => {
                        script.push('\\');
                        script.push(c);
                    }
                    None => return Err(Refused::Syntax(String::from("a ` with no ` after it"))),
                },
                Some(c) => script.push(c),
            }
        }
    }
}

/// Adds text to a word, to the last part if that is the same kind.
fn push_text(parts: &mut Vec<Part>, text: &str, quoted: bool) {
    match (parts.last_mut(), quoted) {
        (Some(Part::Quoted(last)), true) | (Some(Part::Plain(last)), false) => last.push_str(text),
        _ => parts.push(if quoted { Part::Quoted(String::from(text)) } else { Part::Plain(String::from(text)) }),
    }
}

// ---- the language: commands -----------------------------------------------

/// A script: and-or lists, run one after another.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Script {
    lists: Vec<AndOr>,
}

/// `a && b || c`: pipelines, each run or not by the status before it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AndOr {
    first: Pipeline,
    /// `true` for `&&`, `false` for `||`.
    rest: Vec<(bool, Pipeline)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pipeline {
    negated: bool,
    commands: Vec<Cmd>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Cmd {
    Simple(Simple),
    /// `source`: the command as written, without its redirections (for
    /// a subshell, what is inside the parentheses) - what a copy of the
    /// shell runs when this one cannot.
    Compound { body: Compound, redirections: Vec<Redirect>, source: String },
    /// `name() command`; `source` the whole definition.
    Function { name: String, body: Box<Cmd>, source: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Compound {
    /// `{ list; }`
    Group(Vec<AndOr>),
    /// `( list )`: run by a copy of the shell, so that nothing it does
    /// changes this one.
    Subshell(Vec<AndOr>),
    /// `if list; then list; [elif list; then list;]... [else list;] fi`
    If { arms: Vec<(Vec<AndOr>, Vec<AndOr>)>, otherwise: Option<Vec<AndOr>> },
    /// `for name [in word...]; do list; done`: without `in`, "$@".
    For { name: String, words: Option<Vec<Word>>, body: Vec<AndOr> },
    /// `while list; do list; done`, and `until` (`until`).
    Loop { until: bool, condition: Vec<AndOr>, body: Vec<AndOr> },
    /// `case word in pattern|pattern) list;; ... esac`
    Case { word: Word, arms: Vec<(Vec<Word>, Vec<AndOr>)> },
}

/// `NAME=value ... word ... redirection ...`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Simple {
    assignments: Vec<(String, Word)>,
    words: Vec<Word>,
    redirections: Vec<Redirect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Redirect {
    fd: u32,
    kind: RedirectKind,
    target: Word,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedirectKind {
    In,
    Out,
    Append,
    /// `>&` and `<&`: a number, or `-`.
    Copy,
}

/// Reserved words that only end or continue a compound command: said
/// first in a command where nothing is open, a mistake.
const CLOSERS: [&str; 9] = ["then", "else", "elif", "fi", "do", "done", "esac", "}", "in"];

/// A script, read: its commands, or why it cannot be run.
pub fn parse(script: &str) -> Result<Script, Refused> {
    let chars: Vec<char> = script.chars().collect();
    let tokens = Lexer { chars: chars.clone(), at: 0 }.tokens()?;
    let mut parser = Parser { tokens, chars, at: 0 };
    let lists = parser.list(&[])?;
    match parser.peek() {
        None => Ok(Script { lists }),
        Some(token) => Err(Refused::Syntax(format!("{} where a command should end", describe(token)))),
    }
}

/// A token, as said in a message.
fn describe(token: &Token) -> String {
    match token {
        Token::Operator("\n") => String::from("the end of a line"),
        Token::Operator(op) => format!("'{op}'"),
        Token::Number(n) => format!("'{n}'"),
        Token::Word(word) => format!("'{}'", word.plain().unwrap_or("a word")),
    }
}

struct Parser {
    tokens: Vec<(Token, usize, usize)>,
    chars: Vec<char>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _, _)| token)
    }

    fn peek_at(&self, ahead: usize) -> Option<&Token> {
        self.tokens.get(self.at + ahead).map(|(token, _, _)| token)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.peek().cloned();
        self.at += 1;
        token
    }

    /// The next token's text, if it is a plain word: a reserved word
    /// where one may be.
    fn keyword(&self) -> Option<&str> {
        match self.peek() {
            Some(Token::Word(word)) => word.plain(),
            _ => None,
        }
    }

    fn skip_newlines(&mut self) {
        while self.peek() == Some(&Token::Operator("\n")) {
            self.at += 1;
        }
    }

    /// Where the next token starts in the script.
    fn here(&self) -> usize {
        self.tokens.get(self.at).map_or(self.chars.len(), |&(_, start, _)| start)
    }

    /// The script from `start` to the end of the last token read.
    fn source(&self, start: usize) -> String {
        let end = self.at.checked_sub(1).and_then(|n| self.tokens.get(n)).map_or(start, |&(_, _, end)| end);
        self.chars[start..end.max(start)].iter().collect()
    }

    /// What is found, for a message.
    fn found(&self) -> String {
        self.peek().map_or_else(|| String::from("the end"), describe)
    }

    /// Reserved word `keyword`, next, or a mistake.
    fn expect(&mut self, keyword: &str) -> Result<(), Refused> {
        if self.keyword() == Some(keyword) {
            self.at += 1;
            Ok(())
        } else {
            Err(Refused::Syntax(format!("{} where '{keyword}' should be", self.found())))
        }
    }

    fn expect_operator(&mut self, op: &'static str) -> Result<(), Refused> {
        if self.peek() == Some(&Token::Operator(op)) {
            self.at += 1;
            Ok(())
        } else {
            Err(Refused::Syntax(format!("{} where '{op}' should be", self.found())))
        }
    }

    /// And-or lists, separated by `;` or new lines, up to one of the
    /// reserved words `stops` said first in a command, a `)` or `;;`,
    /// or the end.
    fn list(&mut self, stops: &[&str]) -> Result<Vec<AndOr>, Refused> {
        let mut lists = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                None | Some(Token::Operator(")" | ";;")) => break,
                Some(Token::Word(word)) if word.plain().is_some_and(|text| stops.contains(&text)) => break,
                _ => {}
            }
            lists.push(self.and_or()?);
            match self.peek() {
                Some(Token::Operator(";" | "\n")) => self.at += 1,
                Some(Token::Operator("&")) => return Err(Refused::Missing("running a command in the background, &,")),
                _ => break,
            }
        }
        Ok(lists)
    }

    /// A list that must have something in it, before `keyword`.
    fn some_list(&mut self, stops: &[&str], keyword: &str) -> Result<Vec<AndOr>, Refused> {
        let list = self.list(stops)?;
        if list.is_empty() {
            return Err(Refused::Syntax(format!("{} where a command should be, before '{keyword}'", self.found())));
        }
        Ok(list)
    }

    fn and_or(&mut self) -> Result<AndOr, Refused> {
        let first = self.pipeline()?;
        let mut rest = Vec::new();
        while let Some(Token::Operator(op @ ("&&" | "||"))) = self.peek() {
            let and = *op == "&&";
            self.at += 1;
            self.skip_newlines();
            rest.push((and, self.pipeline()?));
        }
        Ok(AndOr { first, rest })
    }

    fn pipeline(&mut self) -> Result<Pipeline, Refused> {
        let negated = self.keyword() == Some("!");
        if negated {
            self.at += 1;
        }
        let mut commands = vec![self.command()?];
        while self.peek() == Some(&Token::Operator("|")) {
            self.at += 1;
            self.skip_newlines();
            commands.push(self.command()?);
        }
        Ok(Pipeline { negated, commands })
    }

    fn command(&mut self) -> Result<Cmd, Refused> {
        let start = self.here();
        let keyword = self.keyword().map(String::from);
        let body = match keyword.as_deref() {
            _ if self.peek() == Some(&Token::Operator("(")) => {
                self.at += 1;
                let inside = self.here();
                let list = self.some_list(&[], ")")?;
                let source = self.source(inside);
                self.expect_operator(")")?;
                let redirections = self.redirections()?;
                return Ok(Cmd::Compound { body: Compound::Subshell(list), redirections, source });
            }
            Some("if") => self.if_clause()?,
            Some(word @ ("while" | "until")) => {
                let until = word == "until";
                self.at += 1;
                let condition = self.some_list(&["do"], "do")?;
                self.expect("do")?;
                let body = self.some_list(&["done"], "done")?;
                self.expect("done")?;
                Compound::Loop { until, condition, body }
            }
            Some("for") => self.for_clause()?,
            Some("case") => self.case_clause()?,
            Some("{") => {
                self.at += 1;
                let list = self.some_list(&["}"], "}")?;
                self.expect("}")?;
                Compound::Group(list)
            }
            Some(closer) if CLOSERS.contains(&closer) => {
                return Err(Refused::Syntax(format!("'{closer}' where a command should be")));
            }
            Some(name)
                if is_name(name)
                    && self.peek_at(1) == Some(&Token::Operator("("))
                    && self.peek_at(2) == Some(&Token::Operator(")")) =>
            {
                let name = String::from(name);
                self.at += 3;
                self.skip_newlines();
                let body = self.command()?;
                if !matches!(body, Cmd::Compound { .. }) {
                    return Err(Refused::Syntax(format!("the function {name}'s body is not a compound command, such as {{ ...; }}")));
                }
                let source = self.source(start);
                return Ok(Cmd::Function { name, body: Box::new(body), source });
            }
            _ => return Ok(Cmd::Simple(self.simple()?)),
        };
        let source = self.source(start);
        let redirections = self.redirections()?;
        Ok(Cmd::Compound { body, redirections, source })
    }

    fn if_clause(&mut self) -> Result<Compound, Refused> {
        self.at += 1;
        let mut arms = Vec::new();
        loop {
            let condition = self.some_list(&["then"], "then")?;
            self.expect("then")?;
            let body = self.some_list(&["elif", "else", "fi"], "fi")?;
            arms.push((condition, body));
            match self.keyword() {
                Some("elif") => self.at += 1,
                Some("else") => {
                    self.at += 1;
                    let otherwise = self.some_list(&["fi"], "fi")?;
                    self.expect("fi")?;
                    return Ok(Compound::If { arms, otherwise: Some(otherwise) });
                }
                _ => {
                    self.expect("fi")?;
                    return Ok(Compound::If { arms, otherwise: None });
                }
            }
        }
    }

    fn for_clause(&mut self) -> Result<Compound, Refused> {
        self.at += 1;
        let name = match self.next() {
            Some(Token::Word(word)) if word.plain().is_some_and(is_name) => String::from(word.plain().unwrap_or_default()),
            _ => return Err(Refused::Syntax(String::from("no name after 'for'"))),
        };
        let mut words = None;
        if self.peek() == Some(&Token::Operator(";")) {
            self.at += 1;
        } else {
            self.skip_newlines();
            if self.keyword() == Some("in") {
                self.at += 1;
                let mut list = Vec::new();
                while let Some(Token::Word(word)) = self.peek() {
                    list.push(word.clone());
                    self.at += 1;
                }
                match self.next() {
                    Some(Token::Operator(";" | "\n")) => {}
                    _ => return Err(Refused::Syntax(String::from("no ';' or new line after a for's words"))),
                }
                words = Some(list);
            }
        }
        self.skip_newlines();
        self.expect("do")?;
        let body = self.some_list(&["done"], "done")?;
        self.expect("done")?;
        Ok(Compound::For { name, words, body })
    }

    fn case_clause(&mut self) -> Result<Compound, Refused> {
        self.at += 1;
        let word = match self.next() {
            Some(Token::Word(word)) => word,
            _ => return Err(Refused::Syntax(String::from("no word after 'case'"))),
        };
        self.skip_newlines();
        self.expect("in")?;
        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if self.keyword() == Some("esac") {
                self.at += 1;
                return Ok(Compound::Case { word, arms });
            }
            if self.peek() == Some(&Token::Operator("(")) {
                self.at += 1;
            }
            let mut patterns = Vec::new();
            loop {
                match self.next() {
                    Some(Token::Word(pattern)) => patterns.push(pattern),
                    other => {
                        let found = other.as_ref().map_or_else(|| String::from("the end"), describe);
                        return Err(Refused::Syntax(format!("{found} where a case's pattern should be")));
                    }
                }
                if self.peek() != Some(&Token::Operator("|")) {
                    break;
                }
                self.at += 1;
            }
            self.expect_operator(")")?;
            let body = self.list(&["esac"])?;
            arms.push((patterns, body));
            if self.peek() == Some(&Token::Operator(";;")) {
                self.at += 1;
            } else {
                self.skip_newlines();
                self.expect("esac")?;
                return Ok(Compound::Case { word, arms });
            }
        }
    }

    /// The redirections after a compound command.
    fn redirections(&mut self) -> Result<Vec<Redirect>, Refused> {
        let mut redirections = Vec::new();
        loop {
            match self.peek().cloned() {
                Some(Token::Number(fd)) => {
                    self.at += 1;
                    let Some(Token::Operator(op)) = self.next() else {
                        return Err(Refused::Syntax(format!("the descriptor {fd} with no redirection after it")));
                    };
                    self.redirect(&mut redirections, Some(fd), op)?;
                }
                Some(Token::Operator(op @ ("<" | ">" | ">>" | ">|" | "<&" | ">&" | "<<" | "<>"))) => {
                    self.at += 1;
                    self.redirect(&mut redirections, None, op)?;
                }
                _ => return Ok(redirections),
            }
        }
    }

    fn simple(&mut self) -> Result<Simple, Refused> {
        let mut simple = Simple::default();
        loop {
            match self.peek().cloned() {
                Some(Token::Word(word)) => {
                    self.at += 1;
                    if simple.words.is_empty() {
                        if let Some(assignment) = assignment(&word) {
                            simple.assignments.push(assignment);
                            continue;
                        }
                    }
                    simple.words.push(word);
                }
                Some(Token::Number(fd)) => {
                    self.at += 1;
                    let Some(Token::Operator(op)) = self.next() else {
                        return Err(Refused::Syntax(format!("the descriptor {fd} with no redirection after it")));
                    };
                    self.redirect(&mut simple.redirections, Some(fd), op)?;
                }
                Some(Token::Operator(op @ ("<" | ">" | ">>" | ">|" | "<&" | ">&" | "<<" | "<>"))) => {
                    self.at += 1;
                    self.redirect(&mut simple.redirections, None, op)?;
                }
                Some(Token::Operator("(")) if !simple.words.is_empty() => {
                    return Err(Refused::Syntax(String::from("'(' in the middle of a command")));
                }
                _ => break,
            }
        }
        if simple.words.is_empty() && simple.assignments.is_empty() && simple.redirections.is_empty() {
            return Err(Refused::Syntax(format!("{} where a command should be", self.found())));
        }
        Ok(simple)
    }

    fn redirect(&mut self, redirections: &mut Vec<Redirect>, fd: Option<u32>, op: &'static str) -> Result<(), Refused> {
        let (default, kind) = match op {
            "<" => (0, RedirectKind::In),
            ">" | ">|" => (1, RedirectKind::Out),
            ">>" => (1, RedirectKind::Append),
            "<&" => (0, RedirectKind::Copy),
            ">&" => (1, RedirectKind::Copy),
            "<<" => return Err(Refused::Missing("a here-document, <<,")),
            _ => return Err(Refused::Missing("opening for reading and writing, <>,")),
        };
        match self.next() {
            Some(Token::Word(target)) => {
                if kind == RedirectKind::Copy && !target.plain().is_some_and(|t| t == "-" || t.bytes().all(|b| b.is_ascii_digit())) {
                    return Err(Refused::Missing("a redirection to a file with >&"));
                }
                redirections.push(Redirect { fd: fd.unwrap_or(default), kind, target });
                Ok(())
            }
            other => Err(Refused::Syntax(format!(
                "{} after '{op}', where a file should be",
                other.as_ref().map_or_else(|| String::from("the end"), describe)
            ))),
        }
    }
}

/// `NAME=value`, plain before the `=`: the name and the value's word.
fn assignment(word: &Word) -> Option<(String, Word)> {
    let Some(Part::Plain(first)) = word.parts.first() else { return None };
    let equals = first.find('=')?;
    let name = &first[..equals];
    if !is_name(name) {
        return None;
    }
    let mut parts = word.parts.clone();
    let rest = String::from(&first[equals + 1..]);
    if rest.is_empty() {
        parts.remove(0);
    } else {
        parts[0] = Part::Plain(rest);
    }
    Some((String::from(name), Word { parts }))
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

// ---- patterns ---------------------------------------------------------------

/// Whether `text` matches `pattern`: `*`, `?` and `[...]` where not
/// quoted, every other character itself. (case, `${NAME%pattern}`.)
fn matches(pattern: &[(char, bool)], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    // The last `*`, and where in the text it was tried up to.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        let step = match pattern.get(p) {
            Some(&('*', false)) => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some(&('?', false)) => Some(p + 1),
            Some(&('[', false)) => match bracket(pattern, p, text[t]) {
                Some((true, next)) => Some(next),
                Some((false, _)) => None,
                // No `]`: the `[` is itself.
                None => (text[t] == '[').then_some(p + 1),
            },
            Some(&(c, _)) => (c == text[t]).then_some(p + 1),
            None => None,
        };
        match (step, star) {
            (Some(next), _) => {
                p = next;
                t += 1;
            }
            (None, Some((at, tried))) => {
                p = at + 1;
                t = tried + 1;
                star = Some((at, tried + 1));
            }
            (None, None) => return false,
        }
    }
    pattern[p..].iter().all(|&c| c == ('*', false))
}

/// `[...]` at `at` in the pattern: whether `c` is in it, and where the
/// pattern goes on after it; None if it has no `]`.
fn bracket(pattern: &[(char, bool)], at: usize, c: char) -> Option<(bool, usize)> {
    let mut i = at + 1;
    let negated = matches!(pattern.get(i), Some(&('!' | '^', false)));
    if negated {
        i += 1;
    }
    let first = i;
    let mut found = false;
    loop {
        let &(here, quoted) = pattern.get(i)?;
        if here == ']' && !quoted && i > first {
            return Some((found != negated, i + 1));
        }
        match (pattern.get(i + 1), pattern.get(i + 2)) {
            (Some(&('-', false)), Some(&(high, _))) if high != ']' => {
                found |= (here..=high).contains(&c);
                i += 3;
            }
            _ => {
                found |= here == c;
                i += 1;
            }
        }
    }
}

// ---- arithmetic -------------------------------------------------------------

/// `$((...))`: whole numbers, with C's operators but assignment and
/// `?:`; a name, or `$NAME`, is the variable's value (0 if unset or
/// empty).
struct Arithmetic<'a> {
    tokens: Vec<Atom>,
    at: usize,
    lookup: &'a dyn Fn(&str) -> Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Atom {
    Number(i64),
    Name(String),
    Operator(&'static str),
}

/// The binary operators, from the loosest to the tightest.
const LEVELS: [&[&str]; 10] =
    [&["||"], &["&&"], &["|"], &["^"], &["&"], &["==", "!="], &["<=", ">=", "<", ">"], &["<<", ">>"], &["+", "-"], &["*", "/", "%"]];
const ARITHMETIC_OPERATORS: [&str; 22] =
    ["<<", ">>", "<=", ">=", "==", "!=", "&&", "||", "<", ">", "+", "-", "*", "/", "%", "(", ")", "!", "~", "&", "|", "^"];

/// A number as C writes one: `0x1f`, `017`, `15`.
fn number(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let value = if let Some(hex) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).ok()?
    } else if digits.len() > 1 && digits.starts_with('0') {
        i64::from_str_radix(&digits[1..], 8).ok()?
    } else {
        digits.parse().ok()?
    };
    Some(if negative { value.wrapping_neg() } else { value })
}

fn arithmetic(expression: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<i64, String> {
    let chars: Vec<char> = expression.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            tokens.push(Atom::Number(number(&text).ok_or_else(|| format!("{text}: not a number"))?));
        } else if c == '_' || c.is_ascii_alphabetic() || c == '$' {
            if c == '$' {
                i += 1;
                if chars.get(i) == Some(&'(') {
                    return Err(String::from("a command substitution in $((...)) is not in this shell yet"));
                }
            }
            let braced = chars.get(i) == Some(&'{');
            if braced {
                i += 1;
            }
            let start = i;
            while i < chars.len() && (chars[i] == '_' || chars[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            if start == i && i < chars.len() && "?#".contains(chars[i]) {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            if braced {
                if chars.get(i) != Some(&'}') {
                    return Err(String::from("a ${ with no } after it"));
                }
                i += 1;
            }
            if name.is_empty() {
                return Err(String::from("a $ with no name after it"));
            }
            tokens.push(Atom::Name(name));
        } else {
            let rest: String = chars[i..].iter().collect();
            match ARITHMETIC_OPERATORS.iter().find(|op| rest.starts_with(**op)) {
                Some(op) => {
                    i += op.len();
                    tokens.push(Atom::Operator(op));
                }
                None if c == '=' => return Err(String::from("assignment in $((...)) is not in this shell yet")),
                None => return Err(format!("'{c}' is not an operator")),
            }
        }
    }
    let mut parser = Arithmetic { tokens, at: 0, lookup };
    let value = parser.binary(0)?;
    match parser.tokens.get(parser.at) {
        None => Ok(value),
        Some(token) => Err(format!("{token:?} where the expression should end")),
    }
}

impl Arithmetic<'_> {
    fn binary(&mut self, level: usize) -> Result<i64, String> {
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        while let Some(&Atom::Operator(op)) = self.tokens.get(self.at) {
            if !LEVELS[level].contains(&op) {
                break;
            }
            self.at += 1;
            let right = self.binary(level + 1)?;
            left = match op {
                "||" => i64::from(left != 0 || right != 0),
                "&&" => i64::from(left != 0 && right != 0),
                "|" => left | right,
                "^" => left ^ right,
                "&" => left & right,
                "==" => i64::from(left == right),
                "!=" => i64::from(left != right),
                "<=" => i64::from(left <= right),
                ">=" => i64::from(left >= right),
                "<" => i64::from(left < right),
                ">" => i64::from(left > right),
                "<<" => left.wrapping_shl(right as u32),
                ">>" => left.wrapping_shr(right as u32),
                "+" => left.wrapping_add(right),
                "-" => left.wrapping_sub(right),
                "*" => left.wrapping_mul(right),
                _ if right == 0 => return Err(String::from("division by zero")),
                "/" => left.wrapping_div(right),
                _ => left.wrapping_rem(right),
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<i64, String> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        match token {
            Some(Atom::Number(n)) => Ok(n),
            Some(Atom::Name(name)) => match (self.lookup)(&name) {
                None => Ok(0),
                Some(value) if value.trim().is_empty() => Ok(0),
                Some(value) => number(&value).ok_or_else(|| format!("{name}: {value}: not a number")),
            },
            Some(Atom::Operator("-")) => Ok(self.unary()?.wrapping_neg()),
            Some(Atom::Operator("+")) => self.unary(),
            Some(Atom::Operator("!")) => Ok(i64::from(self.unary()? == 0)),
            Some(Atom::Operator("~")) => Ok(!self.unary()?),
            Some(Atom::Operator("(")) => {
                let value = self.binary(0)?;
                match self.tokens.get(self.at) {
                    Some(Atom::Operator(")")) => {
                        self.at += 1;
                        Ok(value)
                    }
                    _ => Err(String::from("a ( with no ) after it")),
                }
            }
            Some(other) => Err(format!("{other:?} where a number should be")),
            None => Err(String::from("the end where a number should be")),
        }
    }
}

// ---- the shell -------------------------------------------------------------

struct Variable {
    name: String,
    value: String,
    exported: bool,
}

struct Function {
    name: String,
    body: Cmd,
    /// The definition as written: what a copy of the shell is given.
    source: String,
}

/// What `break`, `continue` and `return` leave for the commands around
/// them to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// Out of this many loops.
    Break(u32),
    /// To the next round of the loop this many out.
    Continue(u32),
    Return,
}

/// A shell: its variables, functions, arguments and options, and the
/// system it runs commands on.
pub struct Shell<S: System> {
    pub system: S,
    variables: Vec<Variable>,
    functions: Vec<Function>,
    /// `$0`, and `$1` on.
    zero: String,
    arguments: Vec<String>,
    /// `$?`
    status: i32,
    /// `set -e`: stop at a command that fails.
    errexit: bool,
    /// `set -x`: each command said on standard error before it runs.
    xtrace: bool,
    /// Set by `exit`, or by `set -e` stopping: the shell's status.
    exited: Option<i32>,
    control: Option<Flow>,
    /// How many loops are running, and functions, and conditions (an
    /// if's or a while's, where set -e does not stop the shell).
    loops: u32,
    calls: u32,
    conditions: u32,
    /// The status of the last command substitution in the command
    /// being expanded: a command with no words has it as its own.
    substituted: Option<i32>,
    /// An expansion failed (`$((1/0))`): the command is not run.
    failed: bool,
}

/// The builtins that change the shell itself: run here, always.
const SPECIAL: [&str; 12] = ["cd", "exit", "export", "unset", "set", "shift", "read", "eval", "return", "break", "continue", "."];
/// The builtins that only say something: run here when alone, and by
/// another copy of the shell in a pipeline or with redirections.
const REGULAR: [&str; 7] = [":", "true", "false", "echo", "pwd", "test", "["];

/// Whether `name` is one of this shell's builtins.
pub fn is_builtin(name: &str) -> bool {
    SPECIAL.contains(&name) || REGULAR.contains(&name)
}

/// Splits on these, if IFS is not set.
const IFS: &str = " \t\n";

/// The script a copy of the shell runs for a builtin or a function:
/// its arguments, as a command.
const ITS_ARGUMENTS: &str = "\"$@\"";

impl<S: System> Shell<S> {
    /// A shell with `environment` (`NAME=value` strings) as its exported
    /// variables, `zero` as `$0` and `arguments` as `$1` on.
    pub fn new(system: S, environment: &[String], zero: String, arguments: Vec<String>) -> Self {
        let variables = environment
            .iter()
            .filter_map(|entry| entry.split_once('='))
            .filter(|(name, _)| is_name(name))
            .map(|(name, value)| Variable { name: String::from(name), value: String::from(value), exported: true })
            .collect();
        Shell {
            system,
            variables,
            functions: Vec::new(),
            zero,
            arguments,
            status: 0,
            errexit: false,
            xtrace: false,
            exited: None,
            control: None,
            loops: 0,
            calls: 0,
            conditions: 0,
            substituted: None,
            failed: false,
        }
    }

    pub fn set_errexit(&mut self, on: bool) {
        self.errexit = on;
    }

    pub fn set_xtrace(&mut self, on: bool) {
        self.xtrace = on;
    }

    /// Runs a script: the shell's status after it (`exit`'s, if it
    /// ran). One that cannot be read is not run at all: status 2, and
    /// why, on standard error.
    pub fn run(&mut self, script: &str) -> i32 {
        match parse(script) {
            Ok(script) => {
                self.run_list(&script.lists);
                // A return outside a function, or a break outside a
                // loop, ends here.
                self.control = None;
                self.exited.unwrap_or(self.status)
            }
            Err(refused) => {
                self.complain(&refused.describe());
                self.status = 2;
                2
            }
        }
    }

    fn complain(&mut self, text: &str) {
        self.system.write(2, &format!("sh: {text}\n"));
    }

    /// Whether the commands that follow are not to run: the shell
    /// exited, or a break, continue or return is on its way out.
    fn stopping(&self) -> bool {
        self.exited.is_some() || self.control.is_some()
    }

    fn run_list(&mut self, lists: &[AndOr]) -> i32 {
        if lists.is_empty() {
            self.status = 0;
        }
        for list in lists {
            self.run_and_or(list);
            if self.stopping() {
                break;
            }
        }
        self.status
    }

    fn run_and_or(&mut self, list: &AndOr) {
        let mut status = self.run_pipeline(&list.first);
        // Whether the status is the list's last pipeline's, and that
        // one not negated: only then does set -e stop the shell.
        let mut counts = list.rest.is_empty() && !list.first.negated;
        for (n, (and, pipeline)) in list.rest.iter().enumerate() {
            if self.stopping() {
                return;
            }
            if (status == 0) == *and {
                status = self.run_pipeline(pipeline);
                counts = n + 1 == list.rest.len() && !pipeline.negated;
            }
        }
        self.status = status;
        if self.errexit && counts && status != 0 && self.conditions == 0 && !self.stopping() {
            self.exited = Some(status);
        }
    }

    fn run_pipeline(&mut self, pipeline: &Pipeline) -> i32 {
        let status = if let [command] = pipeline.commands.as_slice() {
            self.run_command(command)
        } else {
            let mut commands = Vec::new();
            for command in &pipeline.commands {
                let prepared = self.prepare(command);
                commands.push(prepared);
            }
            self.trace(&commands);
            self.system.pipeline(&commands)
        };
        if self.stopping() && pipeline.negated {
            return self.status;
        }
        self.status = if pipeline.negated { i32::from(status == 0) } else { status };
        self.status
    }

    /// A command of a pipeline, as the system is to start it: a
    /// program, or a copy of this shell for anything else.
    fn prepare(&mut self, command: &Cmd) -> Command {
        match command {
            Cmd::Simple(simple) => {
                let words = self.fields(&simple.words);
                if words.is_empty() {
                    // Nothing to run (`a | X=1 | b`): a command that
                    // does nothing stands in its place.
                    let redirections = self.redirections(&simple.redirections);
                    return self.copy_running(vec![String::from(":")], redirections, Vec::new());
                }
                self.prepare_with(simple, words)
            }
            Cmd::Compound { redirections, source, .. } => {
                let redirections = self.redirections(redirections);
                self.compound_copy(source, redirections)
            }
            Cmd::Function { .. } => self.copy_running(vec![String::from(":")], Vec::new(), Vec::new()),
        }
    }

    /// A simple command, its words already expanded.
    fn prepare_with(&mut self, simple: &Simple, words: Vec<String>) -> Command {
        let assigned: Vec<(String, String)> =
            simple.assignments.iter().map(|(name, word)| (name.clone(), self.single(word))).collect();
        let redirections = self.redirections(&simple.redirections);
        if is_builtin(&words[0]) || self.function(&words[0]).is_some() {
            return self.copy_running(words, redirections, assigned);
        }
        Command { environment: self.environment(&assigned), words, redirections, shell: None }
    }

    /// A copy of this shell, running `words` as a command: a builtin's,
    /// or a function's.
    fn copy_running(&self, words: Vec<String>, redirections: Vec<Redirection>, assigned: Vec<(String, String)>) -> Command {
        let copy = Copy { functions: self.definitions(), script: String::from(ITS_ARGUMENTS), zero: String::from("sh") };
        Command { words, redirections, environment: self.everything(&assigned), shell: Some(copy) }
    }

    /// A copy of this shell, running `source`, with this one's `$0` and
    /// arguments.
    fn compound_copy(&self, source: &str, redirections: Vec<Redirection>) -> Command {
        let copy = Copy { functions: self.definitions(), script: String::from(source), zero: self.zero.clone() };
        Command { words: self.arguments.clone(), redirections, environment: self.everything(&[]), shell: Some(copy) }
    }

    /// The functions, as a script defining them.
    fn definitions(&self) -> String {
        self.functions.iter().map(|function| format!("{}\n", function.source)).collect()
    }

    fn function(&self, name: &str) -> Option<usize> {
        self.functions.iter().position(|function| function.name == name)
    }

    fn redirections(&mut self, redirects: &[Redirect]) -> Vec<Redirection> {
        redirects
            .iter()
            .map(|redirect| {
                let target = self.single(&redirect.target);
                let fd = redirect.fd;
                match redirect.kind {
                    RedirectKind::In => Redirection::In { fd, path: target },
                    RedirectKind::Out => Redirection::Out { fd, path: target, append: false },
                    RedirectKind::Append => Redirection::Out { fd, path: target, append: true },
                    RedirectKind::Copy if target == "-" => Redirection::Close { fd },
                    RedirectKind::Copy => Redirection::Copy { fd, to: target.parse().unwrap_or(0) },
                }
            })
            .collect()
    }

    fn trace(&mut self, commands: &[Command]) {
        if self.xtrace {
            let line: Vec<String> = commands
                .iter()
                .map(|command| match &command.shell {
                    Some(copy) if copy.script != ITS_ARGUMENTS => copy.script.clone(),
                    _ => command.words.join(" "),
                })
                .collect();
            self.system.write(2, &format!("+ {}\n", line.join(" | ")));
        }
    }

    fn run_command(&mut self, command: &Cmd) -> i32 {
        match command {
            Cmd::Simple(simple) => self.run_simple(simple),
            Cmd::Compound { body, redirections, source } => {
                if matches!(body, Compound::Subshell(_)) || !redirections.is_empty() {
                    let redirections = self.redirections(redirections);
                    let copy = self.compound_copy(source, redirections);
                    self.trace(core::slice::from_ref(&copy));
                    self.status = self.system.pipeline(&[copy]);
                    return self.status;
                }
                self.run_compound(body)
            }
            Cmd::Function { name, body, source } => {
                let function = Function { name: name.clone(), body: (**body).clone(), source: source.clone() };
                match self.function(name) {
                    Some(n) => self.functions[n] = function,
                    None => self.functions.push(function),
                }
                self.status = 0;
                0
            }
        }
    }

    /// After a loop's body or condition: whether the loop ends here,
    /// and what a break or continue leaves for the loops around it.
    fn loop_ends(&mut self) -> bool {
        match self.control.take() {
            None => self.exited.is_some(),
            Some(Flow::Break(n)) => {
                if n > 1 {
                    self.control = Some(Flow::Break(n - 1));
                }
                true
            }
            Some(Flow::Continue(n)) => {
                if n > 1 {
                    self.control = Some(Flow::Continue(n - 1));
                }
                n > 1
            }
            Some(Flow::Return) => {
                self.control = Some(Flow::Return);
                true
            }
        }
    }

    /// A condition's list: set -e does not stop the shell in it.
    fn condition(&mut self, list: &[AndOr]) -> i32 {
        self.conditions += 1;
        let status = self.run_list(list);
        self.conditions -= 1;
        status
    }

    fn run_compound(&mut self, body: &Compound) -> i32 {
        match body {
            Compound::Group(list) | Compound::Subshell(list) => self.run_list(list),
            Compound::If { arms, otherwise } => {
                for (condition, then) in arms {
                    let status = self.condition(condition);
                    if self.stopping() {
                        return self.status;
                    }
                    if status == 0 {
                        return self.run_list(then);
                    }
                }
                match otherwise {
                    Some(list) => self.run_list(list),
                    None => {
                        self.status = 0;
                        0
                    }
                }
            }
            Compound::Loop { until, condition, body } => {
                let mut status = 0;
                self.loops += 1;
                loop {
                    let held = self.condition(condition);
                    if self.loop_ends() {
                        break;
                    }
                    if (held == 0) == *until {
                        break;
                    }
                    status = self.run_list(body);
                    if self.loop_ends() {
                        break;
                    }
                }
                self.loops -= 1;
                self.status = status;
                status
            }
            Compound::For { name, words, body } => {
                let values = match words {
                    Some(words) => self.fields(words),
                    None => self.arguments.clone(),
                };
                let mut status = 0;
                self.loops += 1;
                for value in values {
                    self.assign(name, value);
                    status = self.run_list(body);
                    if self.loop_ends() {
                        break;
                    }
                }
                self.loops -= 1;
                self.status = status;
                status
            }
            Compound::Case { word, arms } => {
                let subject: Vec<char> = self.single(word).chars().collect();
                for (patterns, list) in arms {
                    for pattern in patterns {
                        let pattern = self.pattern(pattern);
                        if matches(&pattern, &subject) {
                            self.status = 0;
                            return self.run_list(list);
                        }
                    }
                }
                self.status = 0;
                0
            }
        }
    }

    fn run_simple(&mut self, simple: &Simple) -> i32 {
        self.substituted = None;
        let words = self.fields(&simple.words);
        if self.failed {
            // POSIX: an expansion's error ends a shell that is not
            // interactive.
            self.failed = false;
            self.status = 2;
            self.exited = Some(2);
            return 2;
        }
        if words.is_empty() {
            for (name, word) in &simple.assignments {
                let value = self.single(word);
                self.assign(name, value);
            }
            if simple.redirections.is_empty() {
                // A command substitution's status is the command's, if
                // it has no words of its own.
                self.status = self.substituted.unwrap_or(0);
                return self.status;
            }
            // `> file` by itself: the file made, and nothing run.
            let redirections = self.redirections(&simple.redirections);
            let command = self.copy_running(vec![String::from(":")], redirections, Vec::new());
            self.trace(core::slice::from_ref(&command));
            self.status = self.system.pipeline(&[command]);
            return self.status;
        }
        let name = words[0].as_str();
        let here = SPECIAL.contains(&name)
            || (simple.redirections.is_empty() && (self.function(name).is_some() || REGULAR.contains(&name)));
        if here {
            if !simple.redirections.is_empty() {
                self.complain(&format!("{name}: a redirection for {name} is not in this shell yet"));
                self.status = 2;
                return 2;
            }
            let line = Command { words: words.clone(), redirections: Vec::new(), environment: Vec::new(), shell: None };
            self.trace(core::slice::from_ref(&line));
            self.status = match self.function(name) {
                Some(n) if !SPECIAL.contains(&name) => self.call(n, &words),
                _ => self.builtin(&words),
            };
            return self.status;
        }
        let command = self.prepare_with(simple, words);
        self.trace(core::slice::from_ref(&command));
        self.status = self.system.pipeline(&[command]);
        self.status
    }

    /// Runs function `n` with `words` (its name, then its arguments).
    fn call(&mut self, n: usize, words: &[String]) -> i32 {
        let body = self.functions[n].body.clone();
        let saved = core::mem::replace(&mut self.arguments, words[1..].to_vec());
        self.calls += 1;
        self.run_command(&body);
        self.calls -= 1;
        self.arguments = saved;
        if self.control == Some(Flow::Return) {
            self.control = None;
        }
        self.status
    }

    // ---- variables ------------------------------------------------------

    fn get(&self, name: &str) -> Option<String> {
        match name {
            "?" => Some(self.status.to_string()),
            "#" => Some(self.arguments.len().to_string()),
            "$" => Some(self.system.pid().to_string()),
            "!" => Some(String::new()),
            "-" => Some(String::from(match (self.errexit, self.xtrace) {
                (true, true) => "ex",
                (true, false) => "e",
                (false, true) => "x",
                (false, false) => "",
            })),
            "0" => Some(self.zero.clone()),
            "@" | "*" => Some(self.arguments.join(" ")),
            _ if name.bytes().all(|b| b.is_ascii_digit()) => {
                name.parse::<usize>().ok().and_then(|n| self.arguments.get(n.checked_sub(1)?)).cloned()
            }
            _ => self.variables.iter().find(|v| v.name == name).map(|v| v.value.clone()),
        }
    }

    fn assign(&mut self, name: &str, value: String) {
        match self.variables.iter_mut().find(|v| v.name == name) {
            Some(variable) => variable.value = value,
            None => self.variables.push(Variable { name: String::from(name), value, exported: false }),
        }
    }

    /// The exported variables, with `assigned` in place of or beside
    /// them: a program's environment.
    fn environment(&self, assigned: &[(String, String)]) -> Vec<String> {
        let mut out: Vec<String> = self
            .variables
            .iter()
            .filter(|v| v.exported && !assigned.iter().any(|(name, _)| *name == v.name))
            .map(|v| format!("{}={}", v.name, v.value))
            .collect();
        out.extend(assigned.iter().map(|(name, value)| format!("{name}={value}")));
        out
    }

    /// Every variable, exported or not, with `assigned`: what a copy of
    /// the shell is given, so that it sees what this one does.
    fn everything(&self, assigned: &[(String, String)]) -> Vec<String> {
        let mut out: Vec<String> = self
            .variables
            .iter()
            .filter(|v| !assigned.iter().any(|(name, _)| *name == v.name))
            .map(|v| format!("{}={}", v.name, v.value))
            .collect();
        out.extend(assigned.iter().map(|(name, value)| format!("{name}={value}")));
        out
    }

    // ---- expansion ------------------------------------------------------

    /// Words expanded into fields: variables and substitutions, then
    /// split where they were not quoted, then wildcards.
    pub fn fields_of(&mut self, script_words: &str) -> Result<Vec<String>, Refused> {
        let tokens = Lexer { chars: script_words.chars().collect(), at: 0 }.tokens()?;
        let words: Vec<Word> = tokens
            .into_iter()
            .filter_map(|(token, _, _)| match token {
                Token::Word(word) => Some(word),
                _ => None,
            })
            .collect();
        Ok(self.fields(&words))
    }

    fn fields(&mut self, words: &[Word]) -> Vec<String> {
        let mut out = Vec::new();
        for word in words {
            let mut fields = Fields::default();
            self.expand(word, &mut fields, false, false);
            fields.finish();
            for field in fields.done {
                self.wildcards(field, &mut out);
            }
        }
        out
    }

    /// A word expanded into one string, not split and without
    /// wildcards: an assignment's value, a redirection's file.
    fn single(&mut self, word: &Word) -> String {
        self.pattern(word).into_iter().map(|(c, _)| c).collect()
    }

    /// A word expanded as `single` does, each character with whether
    /// it was quoted: a pattern for case or `${NAME%pattern}`.
    fn pattern(&mut self, word: &Word) -> Vec<(char, bool)> {
        let mut fields = Fields::default();
        self.expand(word, &mut fields, true, false);
        fields.finish();
        let mut out = Vec::new();
        for (n, field) in fields.done.into_iter().enumerate() {
            if n > 0 {
                out.push((' ', true));
            }
            out.extend(field);
        }
        out
    }

    /// A value, into the fields: as it is if quoted, else split.
    fn put(fields: &mut Fields, text: &str, quoted: bool, separators: &str) {
        if quoted {
            fields.push(text, true);
        } else {
            fields.split(text, separators);
        }
    }

    /// `whole`: not to be split (quoted, or an assignment's value).
    /// `nested`: the word in `${NAME:-word}`, whose plain text is split
    /// as the value would be.
    fn expand(&mut self, word: &Word, fields: &mut Fields, whole: bool, nested: bool) {
        let separators = self.get("IFS").unwrap_or_else(|| String::from(IFS));
        for part in &word.parts {
            match part {
                Part::Plain(text) if nested && !whole => fields.split(text, &separators),
                Part::Plain(text) => fields.push(text, false),
                Part::Quoted(text) => fields.push(text, true),
                Part::Home => {
                    let home = self.get("HOME").unwrap_or_else(|| String::from("/"));
                    fields.push(&home, true);
                }
                Part::Substitution { script, quoted } => {
                    let copy = Copy { functions: self.definitions(), script: script.clone(), zero: self.zero.clone() };
                    let command = Command {
                        words: self.arguments.clone(),
                        redirections: Vec::new(),
                        environment: self.everything(&[]),
                        shell: Some(copy),
                    };
                    let (output, status) = self.system.capture(&command);
                    self.substituted = Some(status);
                    Self::put(fields, output.trim_end_matches('\n'), *quoted || whole, &separators);
                }
                Part::Arithmetic { expression, quoted } => {
                    let lookup = |name: &str| self.get(name);
                    match arithmetic(expression, &lookup) {
                        Ok(value) => Self::put(fields, &value.to_string(), *quoted || whole, &separators),
                        Err(why) => {
                            self.complain(&format!("$(({expression})): {why}"));
                            self.failed = true;
                        }
                    }
                }
                Part::Parameter { name, op, quoted } => {
                    let quoted = *quoted || whole;
                    if (name == "@" || name == "*") && *op == Op::Value {
                        let arguments = self.arguments.clone();
                        if quoted && name == "@" {
                            for (n, argument) in arguments.iter().enumerate() {
                                if n > 0 {
                                    fields.end();
                                }
                                fields.push(argument, true);
                            }
                        } else if quoted {
                            let first = separators.chars().next().map(String::from).unwrap_or_default();
                            fields.push(&arguments.join(&first), true);
                        } else {
                            for argument in &arguments {
                                fields.split(argument, &separators);
                                fields.end();
                            }
                        }
                        continue;
                    }
                    let value = self.get(name);
                    let unset = |colon: bool| value.as_ref().is_none_or(|v| colon && v.is_empty());
                    let text = match op {
                        Op::Value => value.clone(),
                        Op::Length => Some(value.clone().unwrap_or_default().chars().count().to_string()),
                        Op::Default { colon, word } => {
                            if unset(*colon) {
                                self.expand(word, fields, quoted, true);
                                None
                            } else {
                                value.clone()
                            }
                        }
                        Op::Assign { colon, word } => {
                            if unset(*colon) {
                                let new = self.single(word);
                                if is_name(name) {
                                    self.assign(name, new.clone());
                                }
                                Some(new)
                            } else {
                                value.clone()
                            }
                        }
                        Op::Alternative { colon, word } => {
                            if !unset(*colon) {
                                self.expand(word, fields, quoted, true);
                            }
                            None
                        }
                        Op::Remove { prefix, longest, pattern } => {
                            let pattern = self.pattern(pattern);
                            let value: Vec<char> = value.clone().unwrap_or_default().chars().collect();
                            Some(remove(&value, &pattern, *prefix, *longest))
                        }
                    };
                    if let Some(text) = text {
                        Self::put(fields, &text, quoted, &separators);
                    }
                }
            }
        }
    }

    /// A field, with its wildcards (outside quotes) matched against the
    /// files: the paths, or the field itself if none match.
    fn wildcards(&mut self, field: Vec<(char, bool)>, out: &mut Vec<String>) {
        let text: String = field.iter().map(|&(c, _)| c).collect();
        if !field.iter().any(|&(c, quoted)| !quoted && matches!(c, '*' | '?' | '[')) {
            out.push(text);
            return;
        }
        let mut pattern = String::new();
        for &(c, quoted) in &field {
            if c == '\\' || (quoted && matches!(c, '*' | '?' | '[')) {
                pattern.push('\\');
            }
            pattern.push(c);
        }
        let found = self.system.glob(&pattern);
        if found.is_empty() {
            out.push(text);
        } else {
            out.extend(found);
        }
    }

    // ---- builtins -------------------------------------------------------

    /// Runs builtin `words[0]` here: its status.
    pub fn builtin(&mut self, words: &[String]) -> i32 {
        let arguments = &words[1..];
        match words[0].as_str() {
            ":" | "true" => 0,
            "false" => 1,
            "echo" => {
                let (newline, arguments) = match arguments.first().map(String::as_str) {
                    Some("-n") => (false, &arguments[1..]),
                    _ => (true, arguments),
                };
                let mut line = arguments.join(" ");
                if newline {
                    line.push('\n');
                }
                self.system.write(1, &line);
                0
            }
            "pwd" => {
                let cwd = self.system.cwd();
                self.system.write(1, &format!("{cwd}\n"));
                0
            }
            "cd" => self.cd(arguments),
            "exit" => {
                let status = match self.count(arguments, "exit") {
                    Ok(None) => self.status,
                    Ok(Some(n)) => n & 0xff,
                    Err(status) => status,
                };
                self.exited = Some(status);
                status
            }
            "return" => {
                let status = match self.count(arguments, "return") {
                    Ok(None) => self.status,
                    Ok(Some(n)) => n & 0xff,
                    Err(status) => status,
                };
                if self.calls > 0 {
                    self.control = Some(Flow::Return);
                } else {
                    // Outside a function: the end of the script.
                    self.exited = Some(status);
                }
                status
            }
            loop_word @ ("break" | "continue") => {
                let n = match self.count(arguments, loop_word) {
                    Ok(None) => 1,
                    Ok(Some(n)) if n >= 1 => n as u32,
                    Ok(Some(_)) => {
                        self.complain(&format!("{loop_word}: the count must be 1 or more"));
                        return 1;
                    }
                    Err(status) => return status,
                };
                if self.loops == 0 {
                    self.complain(&format!("{loop_word}: only in a loop"));
                    return 0;
                }
                let n = n.min(self.loops);
                self.control = Some(if loop_word == "break" { Flow::Break(n) } else { Flow::Continue(n) });
                0
            }
            "eval" => {
                let script = arguments.join(" ");
                match parse(&script) {
                    Ok(script) => self.run_list(&script.lists),
                    Err(refused) => {
                        self.complain(&format!("eval: {}", refused.describe()));
                        2
                    }
                }
            }
            "." => {
                self.complain(".: reading a script into this shell is not in this shell yet");
                2
            }
            "read" => self.read(arguments),
            "export" => {
                if arguments.is_empty() {
                    let lines: String = self
                        .variables
                        .iter()
                        .filter(|v| v.exported)
                        .map(|v| format!("export {}={}\n", v.name, quote(&v.value)))
                        .collect();
                    self.system.write(1, &lines);
                    return 0;
                }
                let mut status = 0;
                for argument in arguments {
                    let (name, value) = match argument.split_once('=') {
                        Some((name, value)) => (name, Some(String::from(value))),
                        None => (argument.as_str(), None),
                    };
                    if !is_name(name) {
                        self.complain(&format!("export: {name}: not a name"));
                        status = 1;
                        continue;
                    }
                    if let Some(value) = value {
                        self.assign(name, value);
                    }
                    match self.variables.iter_mut().find(|v| v.name == name) {
                        Some(variable) => variable.exported = true,
                        None => self.variables.push(Variable { name: String::from(name), value: String::new(), exported: true }),
                    }
                }
                status
            }
            "unset" => {
                let functions = arguments.first().is_some_and(|a| a == "-f");
                for name in arguments.iter().filter(|a| *a != "-v" && *a != "-f") {
                    if functions {
                        self.functions.retain(|function| function.name != *name);
                    } else {
                        self.variables.retain(|v| v.name != *name);
                    }
                }
                0
            }
            "set" => self.set(arguments),
            "shift" => {
                let n = match self.count(arguments, "shift") {
                    Ok(None) => 1,
                    Ok(Some(n)) => n as usize,
                    Err(status) => return status,
                };
                if n > self.arguments.len() {
                    self.complain("shift: there are not that many arguments");
                    return 1;
                }
                self.arguments.drain(..n);
                0
            }
            "test" => self.test(arguments),
            "[" => match arguments.split_last() {
                Some((last, rest)) if last == "]" => self.test(rest),
                _ => {
                    self.complain("[: no ] at the end");
                    2
                }
            },
            other => {
                self.complain(&format!("{other}: not a builtin"));
                127
            }
        }
    }

    /// The number a builtin is given, if any; or the status for one
    /// that is not a number.
    fn count(&mut self, arguments: &[String], builtin: &str) -> Result<Option<i32>, i32> {
        match arguments.first() {
            None => Ok(None),
            Some(n) => n.parse::<i32>().map(Some).map_err(|_| {
                self.complain(&format!("{builtin}: {n}: not a number"));
                2
            }),
        }
    }

    /// `read [-r] [NAME...]`: a line of standard input, split as words
    /// are, into the names (REPLY if none), the last taking the rest of
    /// the line. 1 at the end of the input.
    fn read(&mut self, arguments: &[String]) -> i32 {
        let raw = arguments.first().is_some_and(|a| a == "-r");
        let mut names: Vec<String> = arguments.iter().filter(|a| *a != "-r").cloned().collect();
        if names.is_empty() {
            names.push(String::from("REPLY"));
        }
        if let Some(bad) = names.iter().find(|name| !is_name(name)) {
            self.complain(&format!("read: {bad}: not a name"));
            return 2;
        }
        let Some(mut line) = self.system.read_line() else {
            for name in &names {
                self.assign(name, String::new());
            }
            return 1;
        };
        if !raw {
            let mut unescaped = String::new();
            let mut chars = line.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        unescaped.push(next);
                    }
                } else {
                    unescaped.push(c);
                }
            }
            line = unescaped;
        }
        let separators = self.get("IFS").unwrap_or_else(|| String::from(IFS));
        let is_separator = |c: char| separators.contains(c);
        let mut rest = line.trim_matches(is_separator);
        for (n, name) in names.iter().enumerate() {
            let value = if n + 1 == names.len() {
                core::mem::take(&mut rest)
            } else {
                let end = rest.find(is_separator).unwrap_or(rest.len());
                let (field, after) = rest.split_at(end);
                rest = after.trim_start_matches(is_separator);
                field
            };
            let value = String::from(value);
            self.assign(name, value);
        }
        0
    }

    fn cd(&mut self, arguments: &[String]) -> i32 {
        let target = match arguments.first() {
            Some(dir) if dir == "-" => self.get("OLDPWD").unwrap_or_else(|| String::from(".")),
            Some(dir) => dir.clone(),
            None => self.get("HOME").unwrap_or_else(|| String::from("/")),
        };
        let before = self.system.cwd();
        match self.system.chdir(&target) {
            Ok(()) => {
                let now = self.system.cwd();
                self.assign("OLDPWD", before);
                self.assign("PWD", now);
                0
            }
            Err(why) => {
                self.complain(&format!("cd: {target}: {why}"));
                1
            }
        }
    }

    fn set(&mut self, arguments: &[String]) -> i32 {
        if arguments.is_empty() {
            let lines: String = self.variables.iter().map(|v| format!("{}={}\n", v.name, quote(&v.value))).collect();
            self.system.write(1, &lines);
            return 0;
        }
        let mut rest = arguments;
        while let Some(first) = rest.first() {
            if first == "--" {
                self.arguments = rest[1..].to_vec();
                return 0;
            }
            let on = match first.chars().next() {
                Some('-') => true,
                Some('+') => false,
                _ => break,
            };
            for option in first.chars().skip(1) {
                match option {
                    'e' => self.errexit = on,
                    'x' => self.xtrace = on,
                    _ => {
                        self.complain(&format!("set: -{option}: not an option this shell has"));
                        return 2;
                    }
                }
            }
            rest = &rest[1..];
        }
        if !rest.is_empty() {
            self.arguments = rest.to_vec();
        }
        0
    }

    /// `test`'s expression: 0 true, 1 false, 2 not understood.
    fn test(&mut self, arguments: &[String]) -> i32 {
        let strings: Vec<&str> = arguments.iter().map(String::as_str).collect();
        match self.truth(&strings) {
            Some(true) => 0,
            Some(false) => 1,
            None => {
                self.complain(&format!("test: {}: not understood", strings.join(" ")));
                2
            }
        }
    }

    fn truth(&mut self, arguments: &[&str]) -> Option<bool> {
        match arguments {
            [] => Some(false),
            ["!", rest @ ..] if !rest.is_empty() && !(rest.len() == 2 && is_binary(rest[0])) => self.truth(rest).map(|t| !t),
            [one] => Some(!one.is_empty()),
            [op, operand] => self.unary(op, operand),
            [left, op, right] if is_binary(op) => binary(left, op, right),
            _ => None,
        }
    }

    fn unary(&mut self, op: &str, operand: &str) -> Option<bool> {
        Some(match op {
            "-n" => !operand.is_empty(),
            "-z" => operand.is_empty(),
            "-e" => self.system.kind(operand).is_some(),
            "-f" => matches!(self.system.kind(operand), Some(Kind::File { .. })),
            "-d" => matches!(self.system.kind(operand), Some(Kind::Directory)),
            "-s" => matches!(self.system.kind(operand), Some(Kind::File { size }) if size > 0),
            // Every file can be read and written, and run if it is a
            // program: there are no permissions to say otherwise.
            "-r" | "-w" | "-x" => self.system.kind(operand).is_some(),
            _ => return None,
        })
    }
}

/// `${NAME%pattern}` and the rest: `value` without the shortest (or
/// `longest`) end - or, with `prefix`, start - that `pattern` matches.
fn remove(value: &[char], pattern: &[(char, bool)], prefix: bool, longest: bool) -> String {
    let n = value.len();
    let cut = if prefix {
        let mut lengths: Vec<usize> = (0..=n).collect();
        if longest {
            lengths.reverse();
        }
        lengths.into_iter().find(|&len| matches(pattern, &value[..len])).map(|len| (len, n))
    } else {
        let mut starts: Vec<usize> = (0..=n).rev().collect();
        if longest {
            starts.reverse();
        }
        starts.into_iter().find(|&start| matches(pattern, &value[start..])).map(|start| (0, start))
    };
    let (from, to) = cut.unwrap_or((0, n));
    value[from..to].iter().collect()
}

fn is_binary(op: &str) -> bool {
    matches!(op, "=" | "!=" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge")
}

fn binary(left: &str, op: &str, right: &str) -> Option<bool> {
    match op {
        "=" => Some(left == right),
        "!=" => Some(left != right),
        _ => {
            let (l, r) = (left.trim().parse::<i64>().ok()?, right.trim().parse::<i64>().ok()?);
            Some(match op {
                "-eq" => l == r,
                "-ne" => l != r,
                "-lt" => l < r,
                "-le" => l <= r,
                "-gt" => l > r,
                _ => l >= r,
            })
        }
    }
}

/// A value as sh would read it back: in single quotes if it needs them.
fn quote(value: &str) -> String {
    if !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || "_-./:=+,@%".contains(c)) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Fields being made: characters, each with whether it was quoted.
#[derive(Default)]
struct Fields {
    done: Vec<Vec<(char, bool)>>,
    current: Vec<(char, bool)>,
    /// Whether there is a current field, even an empty one ("").
    live: bool,
}

impl Fields {
    fn push(&mut self, text: &str, quoted: bool) {
        self.current.extend(text.chars().map(|c| (c, quoted)));
        // Plain text that is empty makes no field: `${X:-}` alone.
        if quoted || !text.is_empty() {
            self.live = true;
        }
    }

    /// An expansion outside quotes: split at the separators.
    fn split(&mut self, text: &str, separators: &str) {
        for c in text.chars() {
            if separators.contains(c) {
                self.end();
            } else {
                self.current.push((c, false));
                self.live = true;
            }
        }
    }

    fn end(&mut self) {
        if self.live {
            self.done.push(core::mem::take(&mut self.current));
            self.live = false;
        }
    }

    fn finish(&mut self) {
        self.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A system that runs nothing: it remembers what it was asked, and
    /// answers from what the test gave it.
    #[derive(Default)]
    struct Pretend {
        ran: Vec<Command>,
        /// What a program exits with, by name (0 if not given).
        statuses: BTreeMap<String, i32>,
        /// What a program prints, by name, when its output is captured.
        outputs: BTreeMap<String, String>,
        captured: Vec<String>,
        /// Standard input's lines, for read.
        input: Vec<String>,
        files: Vec<String>,
        directories: Vec<String>,
        cwd: String,
        out: String,
        err: String,
    }

    impl System for Pretend {
        fn pipeline(&mut self, commands: &[Command]) -> i32 {
            self.ran.extend(commands.iter().cloned());
            let last = &commands[commands.len() - 1];
            if last.shell.is_some() {
                return i32::from(last.words.first().is_some_and(|w| w == "false"));
            }
            if last.words[0] == "nothing" {
                self.err.push_str("sh: nothing: not found\n");
                return 127;
            }
            *self.statuses.get(&last.words[0]).unwrap_or(&0)
        }

        fn capture(&mut self, command: &Command) -> (String, i32) {
            let script = command.shell.as_ref().map(|copy| copy.script.clone()).unwrap_or_default();
            self.captured.push(script.clone());
            let name = script.split(' ').next().unwrap_or("");
            (self.outputs.get(name).cloned().unwrap_or_default(), *self.statuses.get(name).unwrap_or(&0))
        }

        fn glob(&mut self, pattern: &str) -> Vec<String> {
            let mut found: Vec<String> = self.files.iter().filter(|f| matches(pattern.as_bytes(), f.as_bytes())).cloned().collect();
            found.sort();
            found
        }

        fn chdir(&mut self, path: &str) -> Result<(), String> {
            let full = if path.starts_with('/') { String::from(path) } else { format!("{}/{}", self.cwd.trim_end_matches('/'), path) };
            if self.directories.contains(&full) {
                self.cwd = full;
                Ok(())
            } else {
                Err(String::from("no such directory"))
            }
        }

        fn cwd(&mut self) -> String {
            self.cwd.clone()
        }

        fn kind(&mut self, path: &str) -> Option<Kind> {
            if self.directories.iter().any(|d| d == path) {
                Some(Kind::Directory)
            } else if self.files.iter().any(|f| f == path) {
                Some(Kind::File { size: path.len() as u64 - 1 })
            } else {
                None
            }
        }

        fn write(&mut self, fd: u32, text: &str) {
            if fd == 1 { &mut self.out } else { &mut self.err }.push_str(text);
        }

        fn read_line(&mut self) -> Option<String> {
            (!self.input.is_empty()).then(|| self.input.remove(0))
        }

        fn pid(&self) -> i32 {
            42
        }
    }

    /// `*`, `?` and `\` - enough of a wildcard for the tests.
    fn matches(pattern: &[u8], name: &[u8]) -> bool {
        match (pattern.first(), name.first()) {
            (None, None) => true,
            (Some(b'*'), _) => matches(&pattern[1..], name) || (!name.is_empty() && matches(pattern, &name[1..])),
            (Some(b'?'), Some(_)) => matches(&pattern[1..], &name[1..]),
            (Some(b'\\'), Some(&n)) => pattern.get(1) == Some(&n) && matches(&pattern[2..], &name[1..]),
            (Some(&p), Some(&n)) => p == n && matches(&pattern[1..], &name[1..]),
            _ => false,
        }
    }

    fn shell() -> Shell<Pretend> {
        let system = Pretend { cwd: String::from("/t"), directories: vec![String::from("/t"), String::from("/t/sub")], ..Pretend::default() };
        Shell::new(system, &[String::from("HOME=/home"), String::from("CC=tcc")], String::from("sh"), Vec::new())
    }

    fn words(command: &Command) -> Vec<&str> {
        command.words.iter().map(String::as_str).collect()
    }

    fn ran(shell: &Shell<Pretend>) -> Vec<Vec<&str>> {
        shell.system.ran.iter().map(words).collect()
    }

    fn run(script: &str) -> Shell<Pretend> {
        let mut shell = shell();
        shell.run(script);
        shell
    }

    #[test]
    fn words_are_split_and_quoted_as_sh_does() {
        let shell = run("cc  -c x.c\t-o 'a b' \"c d\" e\\ f '' \"\"");
        assert_eq!(ran(&shell), vec![vec!["cc", "-c", "x.c", "-o", "a b", "c d", "e f", "", ""]]);
        let shell = run("echo \"say \\\"hi\\\" \\x\" 'it''s' a\\\nb");
        assert_eq!(shell.system.out, "say \"hi\" \\x its ab\n");
        let shell = run("cc -DX=1 a=b # a comment\n");
        assert_eq!(ran(&shell), vec![vec!["cc", "-DX=1", "a=b"]]);
    }

    #[test]
    fn lists_run_one_after_another() {
        let done = run("a; b\nc ;");
        assert_eq!(ran(&done), vec![vec!["a"], vec!["b"], vec!["c"]]);
        let mut shell = self::shell();
        shell.system.statuses.insert(String::from("bad"), 3);
        assert_eq!(shell.run("bad && skipped; bad || rescued && after"), 0);
        assert_eq!(ran(&shell), vec![vec!["bad"], vec!["bad"], vec!["rescued"], vec!["after"]]);
        let mut shell = self::shell();
        shell.system.statuses.insert(String::from("bad"), 3);
        assert_eq!(shell.run("good &&\n bad && x"), 3, "x skipped: the status is bad's");
        assert_eq!(shell.run("bad && x || y"), 0);
        assert_eq!(ran(&shell), vec![vec!["good"], vec!["bad"], vec!["bad"], vec!["y"]]);
    }

    #[test]
    fn a_status_is_the_last_commands_and_can_be_negated() {
        let mut shell = shell();
        shell.system.statuses.insert(String::from("bad"), 3);
        assert_eq!(shell.run("bad"), 3);
        assert_eq!(shell.run("! bad"), 0);
        assert_eq!(shell.run("! good"), 1);
        assert_eq!(shell.run("bad; echo $?"), 0);
        assert_eq!(shell.system.out, "3\n");
        assert_eq!(shell.run("nothing"), 127);
        assert_eq!(shell.run("exit 5; echo no"), 5);
        assert_eq!(shell.system.out, "3\n");
    }

    #[test]
    fn set_e_stops_at_a_failure_that_is_not_a_condition() {
        let mut shell = shell();
        shell.system.statuses.insert(String::from("bad"), 4);
        assert_eq!(shell.run("set -e; bad && x; ! bad; ! good; bad || y; echo still; bad; echo not"), 4);
        assert_eq!(shell.system.out, "still\n");
        assert_eq!(ran(&shell), vec![vec!["bad"], vec!["bad"], vec!["good"], vec!["bad"], vec!["y"], vec!["bad"]]);
        let mut shell = self::shell();
        shell.system.statuses.insert(String::from("bad"), 4);
        shell.set_errexit(true);
        assert_eq!(shell.run("good && bad; echo not"), 4, "the last of an && list counts");
        assert_eq!(shell.system.out, "");
    }

    #[test]
    fn pipelines_and_redirections() {
        let shell = run("a x | b 2>&1 | c > out 2>err < in >> log 3>&- 1<&0");
        let commands = &shell.system.ran;
        assert_eq!(commands.len(), 3);
        assert_eq!(words(&commands[0]), vec!["a", "x"]);
        assert_eq!(commands[1].redirections, vec![Redirection::Copy { fd: 2, to: 1 }]);
        assert_eq!(
            commands[2].redirections,
            vec![
                Redirection::Out { fd: 1, path: String::from("out"), append: false },
                Redirection::Out { fd: 2, path: String::from("err"), append: false },
                Redirection::In { fd: 0, path: String::from("in") },
                Redirection::Out { fd: 1, path: String::from("log"), append: true },
                Redirection::Close { fd: 3 },
                Redirection::Copy { fd: 1, to: 0 },
            ]
        );
        let shell = run("echo 2 > f; echo \"2\">g");
        assert_eq!(ran(&shell), vec![vec!["echo", "2"], vec!["echo", "2"]], "a number apart, or quoted, is a word");
        assert!(shell.system.ran.iter().all(|c| c.shell.is_some() && c.redirections.len() == 1 && matches!(c.redirections[0], Redirection::Out { fd: 1, .. })));
        let shell = run("X=out; echo hi > $X.txt");
        assert_eq!(shell.system.ran[0].redirections, vec![Redirection::Out { fd: 1, path: String::from("out.txt"), append: false }]);
    }

    #[test]
    fn a_builtin_alone_runs_here_and_in_a_pipeline_elsewhere() {
        let shell = run("echo one two; echo -n three");
        assert_eq!(shell.system.out, "one two\nthree");
        assert!(shell.system.ran.is_empty());
        let shell = run("echo hi | wc");
        assert_eq!(shell.system.ran.len(), 2);
        assert!(shell.system.ran[0].shell.is_some() && shell.system.ran[1].shell.is_none());
        assert_eq!(shell.system.out, "", "not said here: the pipeline's echo says it");
        let shell = run("> made");
        assert_eq!(ran(&shell), vec![vec![":"]]);
        assert_eq!(shell.system.ran[0].redirections, vec![Redirection::Out { fd: 1, path: String::from("made"), append: false }]);
    }

    #[test]
    fn variables_are_set_used_and_exported() {
        let shell = run("X=1 Y='a b'; echo $X \"$Y\" ${X} ${CC} ${NO} end");
        assert_eq!(shell.system.out, "1 a b 1 tcc end\n");
        let shell = run("X=1; cc; export X; cc; Z=9 cc; export W=2; unset CC; cc");
        let env: Vec<&Vec<String>> = shell.system.ran.iter().map(|c| &c.environment).collect();
        assert_eq!(*env[0], vec!["HOME=/home", "CC=tcc"]);
        assert_eq!(*env[1], vec!["HOME=/home", "CC=tcc", "X=1"]);
        assert_eq!(*env[2], vec!["HOME=/home", "CC=tcc", "X=1", "Z=9"]);
        assert_eq!(*env[3], vec!["HOME=/home", "X=1", "W=2"]);
        let shell = run("CC=clang cc; echo $CC");
        assert_eq!(shell.system.ran[0].environment, vec!["HOME=/home", "CC=clang"], "for the command only");
        assert_eq!(shell.system.out, "tcc\n");
        let shell = run("A=x B=$A; echo $B");
        assert_eq!(shell.system.out, "x\n", "assignments in order");
    }

    #[test]
    fn parameters_with_defaults_and_lengths() {
        let shell = run("E=; echo ${NO-d1} ${E-d2}. ${E:-d3} ${CC:-d4} ${NO+a1}. ${CC+a2} ${E:+a3}. ${#CC} ${#NO}");
        assert_eq!(shell.system.out, "d1 . d3 tcc . a2 . 3 0\n");
        let shell = run("echo ${N=set}; echo $N ${M:-\"two words\"} ${M:-$CC.c}");
        assert_eq!(shell.system.out, "set\nset two words tcc.c\n");
        let shell = run("echo \"${NO:-a  b}\" ${NO:-a  b}");
        assert_eq!(shell.system.out, "a  b a b\n");
        let shell = run("echo $$ $- $1 $# $0; set -e; echo $-");
        assert_eq!(shell.system.out, "42 0 sh\ne\n", "$- and $1 empty: no words");
    }

    #[test]
    fn unquoted_expansions_are_split() {
        let shell = run("F='-c   x.c'; cc $F \"$F\" a${F}b; E=; cc $E \"$E\" x$E");
        assert_eq!(ran(&shell), vec![vec!["cc", "-c", "x.c", "-c   x.c", "a-c", "x.cb"], vec!["cc", "", "x"]]);
        let shell = run("IFS=:; P=a:b::c; cc $P");
        assert_eq!(ran(&shell), vec![vec!["cc", "a", "b", "c"]]);
    }

    #[test]
    fn the_arguments_and_quoted_at() {
        let mut shell = Shell::new(Pretend::default(), &[], String::from("me"), vec![String::from("a b"), String::from("c")]);
        shell.run("cc \"$@\"; cc $@; cc \"$*\"; cc \"x$@y\"; shift; cc \"$@\" $#; set -- p q r; cc $2 $#");
        assert_eq!(
            ran(&shell),
            vec![
                vec!["cc", "a b", "c"],
                vec!["cc", "a", "b", "c"],
                vec!["cc", "a b c"],
                vec!["cc", "xa b", "cy"],
                vec!["cc", "c", "1"],
                vec!["cc", "q", "3"]
            ]
        );
        let mut shell = Shell::new(Pretend::default(), &[], String::from("me"), Vec::new());
        shell.run("cc \"$@\" end");
        assert_eq!(ran(&shell), vec![vec!["cc", "end"]], "no arguments: no word at all");
    }

    #[test]
    fn command_substitution() {
        let mut shell = shell();
        shell.system.outputs.insert(String::from("pwd"), String::from("/t/sub\n\n"));
        shell.system.outputs.insert(String::from("list"), String::from("a b\nc\n"));
        shell.run("cd $(pwd); cc `list` \"$(list)\"; X=$(list); echo \"$X\"");
        assert_eq!(shell.system.cwd, "/t/sub");
        assert_eq!(ran(&shell), vec![vec!["cc", "a", "b", "c", "a b\nc"]]);
        assert_eq!(shell.system.out, "a b\nc\n");
        assert_eq!(shell.system.captured, vec!["pwd", "list", "list", "list"]);
        let mut shell = self::shell();
        shell.system.statuses.insert(String::from("fails"), 6);
        assert_eq!(shell.run("X=$(fails)"), 6, "an assignment's status is its substitution's");
        let mut shell = self::shell();
        shell.run("cc $(echo \"(a)\" ')' | x) end");
        assert_eq!(shell.system.captured, vec!["echo \"(a)\" ')' | x"]);
    }

    #[test]
    fn wildcards_match_files_outside_quotes() {
        let mut shell = shell();
        shell.system.files = ["b.o", "a.o", "x.c", "*.z", "a.z"].iter().map(|f| String::from(*f)).collect();
        shell.run("rm *.o; rm '*.o' \"*\".?; rm *.none; rm x.?; P='*.o'; rm $P \"$P\"");
        assert_eq!(
            ran(&shell),
            vec![vec!["rm", "a.o", "b.o"], vec!["rm", "*.o", "*.z"], vec!["rm", "*.none"], vec!["rm", "x.c"], vec!["rm", "a.o", "b.o", "*.o"]],
            "\"*\".?: the quoted * only itself, not a.z or a.o"
        );
    }

    #[test]
    fn cd_pwd_and_home() {
        let mut shell = shell();
        shell.system.directories.push(String::from("/home"));
        assert_eq!(shell.run("cd sub && pwd && cd /nowhere"), 1);
        assert_eq!(shell.system.out, "/t/sub\n");
        assert_eq!(shell.system.err, "sh: cd: /nowhere: no such directory\n");
        shell.run("cd; pwd; cd -; echo ~ ~/x a~ \"~\"");
        assert_eq!(shell.system.out, "/t/sub\n/home\n/home /home/x a~ ~\n");
        assert_eq!(shell.system.cwd, "/t/sub");
    }

    #[test]
    fn test_and_brackets() {
        let mut shell = shell();
        shell.system.files = vec![String::from("full"), String::from("e")];
        let cases = [
            ("test -f full", 0),
            ("test -d full", 1),
            ("[ -d sub ]", 1),
            ("[ -d /t ]", 0),
            ("[ -e none ]", 1),
            ("[ -s full ]", 0),
            ("[ -s e ]", 1),
            ("test a = a", 0),
            ("test a != a", 1),
            ("[ 3 -lt 10 ]", 0),
            ("[ 3 -ge 10 ]", 1),
            ("[ -z '' ]", 0),
            ("[ -n '' ]", 1),
            ("[ x ]", 0),
            ("[ '' ]", 1),
            ("[ ]", 1),
            ("[ ! -f full ]", 1),
            ("[ ! x = y ]", 0),
            ("[ x = y", 2),
            ("[ 1 -lt x ]", 2),
        ];
        for (script, status) in cases {
            assert_eq!(shell.run(script), status, "{script}");
        }
    }

    #[test]
    fn set_x_says_each_command() {
        let shell = run("set -x; X=a; cc $X | wc; echo hi");
        assert_eq!(shell.system.err, "+ cc a | wc\n+ echo hi\n");
    }

    #[test]
    fn what_this_shell_does_not_have_is_refused_before_anything_runs() {
        for (script, what) in [
            ("a; b &", "running a command in the background, &, is not in this shell yet"),
            ("cat << END", "a here-document, <<, is not in this shell yet"),
            ("a <> f", "opening for reading and writing, <>, is not in this shell yet"),
            ("a >& file", "a redirection to a file with >& is not in this shell yet"),
            ("echo ${X?gone}", "${NAME?message} is not in this shell yet"),
        ] {
            let mut shell = shell();
            assert_eq!(shell.run(script), 2, "{script}");
            assert_eq!(shell.system.err, format!("sh: {what}\n"), "{script}");
            assert!(shell.system.ran.is_empty() && shell.system.out.is_empty(), "{script}");
        }
        for (script, what) in [
            ("echo 'open", "a ' with no ' after it"),
            ("echo \"open", "a \" with no \" after it"),
            ("a |", "the end where a command should be"),
            ("| a", "'|' where a command should be"),
            ("a && && b", "'&&' where a command should be"),
            ("a ;; b", "';;' where a command should end"),
            ("a; fi", "'fi' where a command should be"),
            ("if a; then b; done", "'done' where a command should be"),
            ("if a; then b; fi fi", "'fi' where a command should end"),
            ("if a; then fi", "'fi' where a command should be, before 'fi'"),
            ("while a; b; done", "'done' where a command should be"),
            ("for 1 in a; do b; done", "no name after 'for'"),
            ("for x in a b do c; done", "'done' where 'do' should be"),
            ("case x in a) b;; c esac", "'esac' where ')' should be"),
            ("{ a; ", "the end where '}' should be"),
            ("(a", "the end where ')' should be"),
            ("f() echo x", "the function f's body is not a compound command"),
            ("echo $((1+2)", "a $(( closed by one ) only"),
            ("a > ", "the end after '>', where a file should be"),
            ("; a", "';' where a command should be"),
            ("echo ${", "a ${...} with no name in it"),
            ("echo ${X", "a ${ with no } after it"),
            ("echo $(x", "a $( with no ) after it"),
        ] {
            let mut shell = shell();
            assert_eq!(shell.run(script), 2, "{script}");
            assert!(shell.system.err.contains(what), "{script}: {}", shell.system.err);
            assert!(shell.system.ran.is_empty(), "{script}");
        }
        assert!(run("echo if then fi").system.err.is_empty(), "not first, they are words");
        assert_eq!(run("echo if then fi").system.out, "if then fi\n");
    }

    #[test]
    fn export_and_set_say_what_is_set() {
        let shell = run("X='a b'; export Y=1; export; set");
        assert_eq!(shell.system.out, "export HOME=/home\nexport CC=tcc\nexport Y=1\nHOME=/home\nCC=tcc\nX='a b'\nY=1\n");
        assert_eq!(run("export 1x").system.err, "sh: export: 1x: not a name\n");
    }

    fn run_with(script: &str, setup: impl FnOnce(&mut Pretend)) -> Shell<Pretend> {
        let mut shell = shell();
        setup(&mut shell.system);
        shell.run(script);
        shell
    }

    #[test]
    fn if_elif_else() {
        let shell = run_with("if bad; then echo 1; elif good; then echo 2; else echo 3; fi; if bad; then echo 4; fi; echo $?", |s| {
            s.statuses.insert(String::from("bad"), 1);
        });
        assert_eq!(shell.system.out, "2\n0\n");
        let shell = run_with("if bad\nthen\n  echo no\nelse\n  echo else\nfi", |s| {
            s.statuses.insert(String::from("bad"), 1);
        });
        assert_eq!(shell.system.out, "else\n");
        let shell = run("if ! false; then if true; then echo nested; fi; fi");
        assert_eq!(shell.system.out, "nested\n");
    }

    #[test]
    fn for_loops_over_words_and_the_arguments() {
        let mut shell = shell();
        shell.system.files = vec![String::from("a.c"), String::from("b.c")];
        shell.run("for f in *.c 'x y' $NONE; do echo \"[$f]\"; done; echo $f");
        assert_eq!(shell.system.out, "[a.c]\n[b.c]\n[x y]\nx y\n");
        let mut shell = Shell::new(Pretend::default(), &[], String::from("me"), vec![String::from("p"), String::from("q r")]);
        shell.run("for a; do echo $a; done\nfor a\ndo echo \"<$a>\"\ndone");
        assert_eq!(shell.system.out, "p\nq r\n<p>\n<q r>\n");
        let shell = run("for x in; do echo never; done; echo $?");
        assert_eq!(shell.system.out, "0\n");
    }

    #[test]
    fn while_and_until() {
        let shell = run("n=0; while [ $n -lt 3 ]; do echo $n; n=$((n + 1)); done; until [ $n -eq 0 ]; do n=$((n - 1)); done; echo end $n");
        assert_eq!(shell.system.out, "0\n1\n2\nend 0\n");
        let shell = run("while false; do echo no; done; echo $?");
        assert_eq!(shell.system.out, "0\n");
    }

    #[test]
    fn break_and_continue() {
        let shell = run("for i in 1 2 3 4; do if [ $i = 2 ]; then continue; fi; if [ $i = 4 ]; then break; fi; echo $i; done");
        assert_eq!(shell.system.out, "1\n3\n");
        let shell = run("for i in a b; do for j in 1 2; do [ $j = 2 ] && continue 2; [ $i = b ] && break 2; echo $i$j; done; echo never; done; echo out");
        assert_eq!(shell.system.out, "a1\nout\n");
        let shell = run("break; echo after");
        assert_eq!(shell.system.out, "after\n");
        assert_eq!(shell.system.err, "sh: break: only in a loop\n");
    }

    #[test]
    fn case_with_patterns() {
        let script = "for f in main.c x.h Makefile .hidden 'a b'; do case $f in *.c) echo \"$f: C\";; *.h|*.hpp) echo \"$f: header\";; [A-Z]*) echo \"$f: capital\";; '.'*) echo \"$f: dot\";; *) echo \"$f: other\";; esac; done";
        let shell = run(script);
        assert_eq!(shell.system.out, "main.c: C\nx.h: header\nMakefile: capital\n.hidden: dot\na b: other\n");
        let shell = run("case 'a*' in 'a*') echo quoted;; esac; case abc in 'a*') echo no;; a\\*) echo no;; a*) echo yes;; esac");
        assert_eq!(shell.system.out, "quoted\nyes\n");
        let shell = run("case x in\n  (y) echo y ;;\n  ([!y]) echo not-y\nesac\ncase z in y) ;; esac; echo $?");
        assert_eq!(shell.system.out, "not-y\n0\n");
    }

    #[test]
    fn patterns_match_as_sh_does() {
        let cases = [
            ("*", "", true),
            ("a*b", "ab", true),
            ("a*b", "axxb", true),
            ("a*b", "axxbc", false),
            ("*.c", "x.c.c", true),
            ("?", "", false),
            ("??", "ab", true),
            ("[abc]", "b", true),
            ("[!abc]", "b", false),
            ("[a-c]x", "cx", true),
            ("[]]", "]", true),
            ("[", "[", true),
            ("x[", "x[", true),
            ("*[0-9]", "file7", true),
            ("*[0-9]", "file", false),
        ];
        for (pattern, text, expected) in cases {
            let pattern: Vec<(char, bool)> = pattern.chars().map(|c| (c, false)).collect();
            let text: Vec<char> = text.chars().collect();
            assert_eq!(super::matches(&pattern, &text), expected, "{pattern:?} {text:?}");
        }
        assert!(!super::matches(&[('*', true)], &['x']), "a quoted * is itself");
    }

    #[test]
    fn groups_and_subshells() {
        let shell = run("{ echo a; echo b; }; { cd sub; }; pwd");
        assert_eq!(shell.system.out, "a\nb\n/t/sub\n", "a group runs here: its cd stays");
        let shell = run("(cd sub; echo in) ; pwd");
        assert_eq!(shell.system.out, "/t\n", "a subshell is a copy: its output is its own, and its cd too");
        let copy = shell.system.ran[0].shell.as_ref().expect("a copy of the shell");
        assert_eq!(copy.script, "cd sub; echo in");
        let shell = run("{ echo a; } > out; for f in x; do echo $f; done | wc");
        assert_eq!(shell.system.out, "");
        let ran = &shell.system.ran;
        assert_eq!(ran[0].shell.as_ref().map(|c| c.script.as_str()), Some("{ echo a; }"));
        assert_eq!(ran[0].redirections, vec![Redirection::Out { fd: 1, path: String::from("out"), append: false }]);
        assert_eq!(ran[1].shell.as_ref().map(|c| c.script.as_str()), Some("for f in x; do echo $f; done"));
        assert_eq!(words(&ran[2]), vec!["wc"]);
    }

    #[test]
    fn functions() {
        let shell = run("greet() { echo \"hello $1\"; return 3; echo never; }\ngreet you; echo $? $#");
        assert_eq!(shell.system.out, "hello you\n3 0\n");
        let shell = run("f() { for i in 1 2 3; do [ $i = 2 ] && return; echo $i; done; }; f; echo after");
        assert_eq!(shell.system.out, "1\nafter\n");
        let shell = run("f() { echo x; }; f | wc; f > out");
        let ran = &shell.system.ran;
        assert_eq!(ran.len(), 3);
        let copy = ran[0].shell.as_ref().expect("the function, run by a copy");
        assert_eq!(copy.functions, "f() { echo x; }\n", "the copy is given the function");
        assert_eq!(copy.script, "\"$@\"");
        assert_eq!(words(&ran[0]), vec!["f"]);
        assert!(ran[2].shell.is_some() && ran[2].redirections.len() == 1);
        let shell = run("cd() { echo mine; }; cd sub; pwd");
        assert_eq!(shell.system.out, "/t/sub\n", "a special builtin before a function");
        let shell = run("return 4; echo no");
        assert_eq!(shell.system.out, "");
    }

    #[test]
    fn arithmetic_expansion() {
        let shell = run("x=7; echo $((x * 2 + 1)) $(( (x + 1) / 3 )) $((x % 4)) $((-x)) $((x > 5 && x < 10)) $((0x10 + 010)) $(($x<<2)) $((y + 1)) $((!x)) $((~0))");
        assert_eq!(shell.system.out, "15 2 3 -7 1 24 28 1 0 -1\n");
        let mut shell = self::shell();
        assert_eq!(shell.run("echo $((1 / 0)); echo after"), 2);
        assert_eq!(shell.system.out, "");
        assert_eq!(shell.system.err, "sh: $((1 / 0)): division by zero\n");
        let shell = run("x=abc; echo $((x + 1))");
        assert_eq!(shell.system.err, "sh: $((x + 1)): x: abc: not a number\n");
        let shell = run("echo $((x = 1))");
        assert!(shell.system.err.contains("assignment in $((...)) is not in this shell yet"));
    }

    #[test]
    fn removing_patterns_from_values() {
        let shell = run("f=dir/sub/main.tar.gz; echo ${f%.*} ${f%%.*} ${f#*/} ${f##*/} ${f%.zip} ${NONE%x}.");
        assert_eq!(shell.system.out, "dir/sub/main.tar dir/sub/main sub/main.tar.gz main.tar.gz dir/sub/main.tar.gz .\n");
        let shell = run("f='a*b'; echo ${f#'a*'} ${f#a*}");
        assert_eq!(shell.system.out, "b *b\n");
        let shell = run("f='../d/x y.c'; echo \"[${f##*/}]\" \"[${f%.?}]\" \"[${f#'../'}]\"");
        assert_eq!(shell.system.out, "[x y.c] [../d/x y] [d/x y.c]\n", "inside double quotes, a pattern still matches");
    }

    #[test]
    fn read_splits_a_line() {
        let shell = run_with("read a b; echo \"[$a] [$b]\"; read -r line; echo \"$line\"; read x; echo $? \"[$x]\"", |s| {
            s.input = vec![String::from("  one two  three "), String::from("back\\slash")];
        });
        assert_eq!(shell.system.out, "[one] [two  three]\nback\\slash\n1 []\n");
        let shell = run_with("while read l; do echo \"got $l\"; done; echo done", |s| {
            s.input = vec![String::from("x"), String::from("y")];
        });
        assert_eq!(shell.system.out, "got x\ngot y\ndone\n");
        let shell = run("read x < file");
        assert_eq!(shell.system.err, "sh: read: a redirection for read is not in this shell yet\n");
    }

    #[test]
    fn set_e_and_conditions() {
        let mut shell = shell();
        shell.system.statuses.insert(String::from("bad"), 5);
        assert_eq!(shell.run("set -e; if bad; then echo no; fi; while bad; do :; done; echo still; bad; echo not"), 5);
        assert_eq!(shell.system.out, "still\n");
    }

    #[test]
    fn eval_runs_its_arguments() {
        let shell = run("x='echo one; echo two'; eval $x; eval 'y=3'; echo $y");
        assert_eq!(shell.system.out, "one\ntwo\n3\n");
    }
}
