//! A small POSIX shell's language - what `sh -c` and `system` need of
//! one: lists (`;`, `&&`, `||`), pipelines (`|`, `!`), redirections,
//! variables (`NAME=value`, `$NAME`, `${NAME:-word}`, `"$@"`, `$?`),
//! command substitution (`$(...)` and backquotes), quoting, wildcards,
//! and the builtins a recipe uses (cd, echo, test, export, set -e and
//! the rest). Not yet: if, for, while, case, functions, subshells,
//! here-documents, `&`. Those are refused, by name, before anything
//! runs - a script half understood is worse than one refused.
//!
//! No system in it: what it asks of one - start a pipeline, read a
//! directory for a wildcard, change directory - is the `System` trait,
//! which `sh_system.rs` gives it on TheTaaJ. So it is tested on the
//! host, alone (`rustc --test libc/src/sh.rs`, in `make libc-test`),
//! against a pretend system.

extern crate alloc;

use alloc::borrow::ToOwned;
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

/// One command of a pipeline, its words expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub words: Vec<String>,
    pub redirections: Vec<Redirection>,
    /// `NAME=value` strings: what the program's environment is.
    pub environment: Vec<String>,
    /// One of this shell's builtins, to be run by another copy of this
    /// shell (in a pipeline, or with redirections): `words` are the
    /// builtin's.
    pub builtin: bool,
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
    /// Runs `script` in another copy of this shell, with `environment`,
    /// and gives what it wrote to standard output, and its status.
    fn capture(&mut self, script: &str, environment: &[String]) -> (String, i32);
    /// The paths that match `pattern` (a backslash makes the next
    /// character itself), sorted; none if nothing does.
    fn glob(&mut self, pattern: &str) -> Vec<String>;
    fn chdir(&mut self, path: &str) -> Result<(), String>;
    /// The current directory, from the root.
    fn cwd(&mut self) -> String;
    fn kind(&mut self, path: &str) -> Option<Kind>;
    /// Writes to one of this shell's descriptors (1 or 2).
    fn write(&mut self, fd: u32, text: &str);
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

    fn tokens(&mut self) -> Result<Vec<Token>, Refused> {
        let mut tokens = Vec::new();
        loop {
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
                        tokens.push(Token::Operator(op));
                        continue;
                    }
                    let word = self.word()?;
                    let number = word.plain().filter(|text| text.bytes().all(|b| b.is_ascii_digit()));
                    match number {
                        Some(digits) if matches!(self.peek(), Some('<' | '>')) => {
                            let n = digits.parse().map_err(|_| Refused::Syntax(format!("descriptor {digits}")))?;
                            tokens.push(Token::Number(n));
                        }
                        _ => tokens.push(Token::Word(word)),
                    }
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
                    let mut text = String::new();
                    loop {
                        match self.next() {
                            Some('\'') => break,
                            Some(c) => text.push(c),
                            None => return Err(Refused::Syntax(String::from("a ' with no ' after it"))),
                        }
                    }
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
            Some('(') if self.peek_at(1) == Some('(') => Err(Refused::Missing("arithmetic, $((...)),")),
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
        let colon = self.peek() == Some(':');
        if colon {
            self.at += 1;
        }
        let kind = match self.next() {
            Some('}') if !colon => return Ok(Part::Parameter { name, op: Op::Value, quoted }),
            Some(c @ ('-' | '=' | '+')) => c,
            Some('?') => return Err(Refused::Missing("${NAME?message}")),
            Some('%' | '#') => return Err(Refused::Missing("${NAME%pattern} and ${NAME#pattern}")),
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
                    let mut text = String::new();
                    loop {
                        match self.next() {
                            Some('\'') => break,
                            Some(c) => text.push(c),
                            None => return Err(Refused::Syntax(String::from("a ' with no ' after it"))),
                        }
                    }
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
    commands: Vec<Simple>,
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

/// Words that begin what this shell does not have, said first in a
/// command.
const RESERVED: [&str; 14] = ["if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "{", "}"];

/// A script, read: its commands, or why it cannot be run.
pub fn parse(script: &str) -> Result<Script, Refused> {
    let tokens = Lexer { chars: script.chars().collect(), at: 0 }.tokens()?;
    let mut parser = Parser { tokens, at: 0 };
    let mut lists = Vec::new();
    loop {
        parser.skip_newlines();
        if parser.peek().is_none() {
            return Ok(Script { lists });
        }
        lists.push(parser.and_or()?);
        match parser.next() {
            None => return Ok(Script { lists }),
            Some(Token::Operator(";" | "\n")) => {}
            Some(Token::Operator("&")) => return Err(Refused::Missing("running a command in the background, &,")),
            Some(other) => return Err(Refused::Syntax(format!("{} where a command should end", describe(&other)))),
        }
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
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn skip_newlines(&mut self) {
        while self.peek() == Some(&Token::Operator("\n")) {
            self.at += 1;
        }
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
        let negated = matches!(self.peek(), Some(Token::Word(word)) if word.plain() == Some("!"));
        if negated {
            self.at += 1;
        }
        let mut commands = vec![self.simple()?];
        while self.peek() == Some(&Token::Operator("|")) {
            self.at += 1;
            self.skip_newlines();
            commands.push(self.simple()?);
        }
        Ok(Pipeline { negated, commands })
    }

    fn simple(&mut self) -> Result<Simple, Refused> {
        let mut simple = Simple::default();
        loop {
            match self.peek().cloned() {
                Some(Token::Word(word)) => {
                    self.at += 1;
                    if simple.words.is_empty() {
                        if let Some(reserved) = word.plain().and_then(|text| RESERVED.iter().find(|r| **r == text)) {
                            return Err(Refused::Missing(match *reserved {
                                "if" | "then" | "else" | "elif" | "fi" => "if",
                                "for" => "for",
                                "while" | "until" | "do" | "done" => "while",
                                "case" | "esac" => "case",
                                _ => "a group, { ... },",
                            }));
                        }
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
                    self.redirect(&mut simple, Some(fd), op)?;
                }
                Some(Token::Operator(op @ ("<" | ">" | ">>" | ">|" | "<&" | ">&" | "<<" | "<>"))) => {
                    self.at += 1;
                    self.redirect(&mut simple, None, op)?;
                }
                Some(Token::Operator("(" | ")")) => return Err(Refused::Missing("a subshell, ( ... ),")),
                Some(Token::Operator(";;")) => return Err(Refused::Missing("case")),
                _ => break,
            }
        }
        if simple.words.is_empty() && simple.assignments.is_empty() && simple.redirections.is_empty() {
            return Err(Refused::Syntax(match self.peek() {
                None => String::from("the end, where a command should be"),
                Some(token) => format!("{} where a command should be", describe(token)),
            }));
        }
        Ok(simple)
    }

    fn redirect(&mut self, simple: &mut Simple, fd: Option<u32>, op: &'static str) -> Result<(), Refused> {
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
                simple.redirections.push(Redirect { fd: fd.unwrap_or(default), kind, target });
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

// ---- the shell -------------------------------------------------------------

struct Variable {
    name: String,
    value: String,
    exported: bool,
}

/// A shell: its variables, its arguments, its options, and the system
/// it runs commands on.
pub struct Shell<S: System> {
    pub system: S,
    variables: Vec<Variable>,
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
    /// The status of the last command substitution in the command
    /// being expanded: a command with no words has it as its own.
    substituted: Option<i32>,
}

/// The builtins that change the shell itself: run here, always.
const SPECIAL: [&str; 6] = ["cd", "exit", "export", "unset", "set", "shift"];
/// The builtins that only say something: run here when alone, and by
/// another copy of the shell in a pipeline or with redirections.
const REGULAR: [&str; 7] = [":", "true", "false", "echo", "pwd", "test", "["];

/// Whether `name` is one of this shell's builtins.
pub fn is_builtin(name: &str) -> bool {
    SPECIAL.contains(&name) || REGULAR.contains(&name)
}

/// Splits on these, if IFS is not set.
const IFS: &str = " \t\n";

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
        Shell { system, variables, zero, arguments, status: 0, errexit: false, xtrace: false, exited: None, substituted: None }
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
            Ok(script) => self.run_script(&script),
            Err(refused) => {
                self.complain(&refused.describe());
                self.status = 2;
                2
            }
        }
    }

    fn run_script(&mut self, script: &Script) -> i32 {
        for list in &script.lists {
            self.run_and_or(list);
            if let Some(status) = self.exited {
                return status;
            }
        }
        self.status
    }

    fn complain(&mut self, text: &str) {
        self.system.write(2, &format!("sh: {text}\n"));
    }

    fn run_and_or(&mut self, list: &AndOr) {
        let mut status = self.run_pipeline(&list.first);
        // Whether the status is the list's last pipeline's, and that
        // one not negated: only then does set -e stop the shell.
        let mut counts = list.rest.is_empty() && !list.first.negated;
        for (n, (and, pipeline)) in list.rest.iter().enumerate() {
            if self.exited.is_some() {
                return;
            }
            if (status == 0) == *and {
                status = self.run_pipeline(pipeline);
                counts = n + 1 == list.rest.len() && !pipeline.negated;
            }
        }
        self.status = status;
        if self.errexit && counts && status != 0 && self.exited.is_none() {
            self.exited = Some(status);
        }
    }

    fn run_pipeline(&mut self, pipeline: &Pipeline) -> i32 {
        let status = if let [simple] = pipeline.commands.as_slice() {
            self.run_simple(simple)
        } else {
            let mut commands = Vec::new();
            for simple in &pipeline.commands {
                match self.prepare(simple) {
                    Some(command) => commands.push(command),
                    // Nothing to run (`a | X=1 | b`): a command that
                    // does nothing stands in its place.
                    None => commands.push(Command {
                        words: vec![String::from(":")],
                        redirections: Vec::new(),
                        environment: self.environment(&[]),
                        builtin: true,
                    }),
                }
            }
            self.trace(&commands);
            self.system.pipeline(&commands)
        };
        self.status = if pipeline.negated { i32::from(status == 0) } else { status };
        self.status
    }

    /// A command of a pipeline, its words expanded; None if it has no
    /// words.
    fn prepare(&mut self, simple: &Simple) -> Option<Command> {
        let words = self.fields(&simple.words);
        if words.is_empty() {
            return None;
        }
        Some(self.prepare_with(simple, words))
    }

    /// A command, its words already expanded.
    fn prepare_with(&mut self, simple: &Simple, words: Vec<String>) -> Command {
        let assigned: Vec<(String, String)> =
            simple.assignments.iter().map(|(name, word)| (name.clone(), self.single(word))).collect();
        let redirections = self.redirections(&simple.redirections);
        Command { builtin: is_builtin(&words[0]), environment: self.environment(&assigned), words, redirections }
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
            let line: Vec<String> = commands.iter().map(|command| command.words.join(" ")).collect();
            self.system.write(2, &format!("+ {}\n", line.join(" | ")));
        }
    }

    fn run_simple(&mut self, simple: &Simple) -> i32 {
        self.substituted = None;
        let words = self.fields(&simple.words);
        if words.is_empty() {
            for (name, word) in &simple.assignments {
                let value = self.single(word);
                self.assign(name, value);
            }
            if simple.redirections.is_empty() {
                // A command substitution's status is the command's, if
                // it has no words of its own.
                return self.substituted.unwrap_or(0);
            }
            // `> file` by itself: the file made, and nothing run.
            let command = Command {
                words: vec![String::from(":")],
                redirections: self.redirections(&simple.redirections),
                environment: self.environment(&[]),
                builtin: true,
            };
            self.trace(core::slice::from_ref(&command));
            return self.system.pipeline(&[command]);
        }
        let name = words[0].as_str();
        if SPECIAL.contains(&name) || (REGULAR.contains(&name) && simple.redirections.is_empty()) {
            let line = Command { words: words.clone(), redirections: Vec::new(), environment: Vec::new(), builtin: true };
            self.trace(core::slice::from_ref(&line));
            return self.builtin(&words);
        }
        let command = self.prepare_with(simple, words);
        self.trace(core::slice::from_ref(&command));
        self.system.pipeline(&[command])
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

    // ---- expansion ------------------------------------------------------

    /// Words expanded into fields: variables and substitutions, then
    /// split where they were not quoted, then wildcards.
    pub fn fields_of(&mut self, script_words: &str) -> Result<Vec<String>, Refused> {
        let tokens = Lexer { chars: script_words.chars().collect(), at: 0 }.tokens()?;
        let words: Vec<Word> = tokens
            .into_iter()
            .filter_map(|token| match token {
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
        let mut fields = Fields::default();
        self.expand(word, &mut fields, true, false);
        fields.finish();
        let pieces: Vec<String> = fields.done.into_iter().map(|field| field.into_iter().map(|(c, _)| c).collect()).collect();
        pieces.join(" ")
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
                    let environment = self.everything();
                    let (output, status) = self.system.capture(script, &environment);
                    self.substituted = Some(status);
                    let output = output.trim_end_matches('\n');
                    if *quoted || whole {
                        fields.push(output, true);
                    } else {
                        fields.split(output, &separators);
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
                    };
                    if let Some(text) = text {
                        if quoted {
                            fields.push(&text, true);
                        } else {
                            fields.split(&text, &separators);
                        }
                    }
                }
            }
        }
    }

    /// Every variable, exported or not: what a substitution's shell is
    /// given, so that it sees what this one does.
    fn everything(&self) -> Vec<String> {
        self.variables.iter().map(|v| format!("{}={}", v.name, v.value)).collect()
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
                let status = match arguments.first() {
                    None => self.status,
                    Some(n) => match n.parse::<i32>() {
                        Ok(n) => n & 0xff,
                        Err(_) => {
                            self.complain(&format!("exit: {n}: not a number"));
                            2
                        }
                    },
                };
                self.exited = Some(status);
                status
            }
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
                for name in arguments.iter().filter(|a| *a != "-v") {
                    self.variables.retain(|v| v.name != *name);
                }
                0
            }
            "set" => self.set(arguments),
            "shift" => {
                let n = match arguments.first().map(|n| n.parse::<usize>()) {
                    None => 1,
                    Some(Ok(n)) => n,
                    Some(Err(_)) => {
                        self.complain("shift: not a number");
                        return 2;
                    }
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
            if last.builtin {
                return i32::from(last.words[0] == "false");
            }
            if last.words[0] == "nothing" {
                self.err.push_str("sh: nothing: not found\n");
                return 127;
            }
            *self.statuses.get(&last.words[0]).unwrap_or(&0)
        }

        fn capture(&mut self, script: &str, environment: &[String]) -> (String, i32) {
            self.captured.push(String::from(script));
            let _ = environment;
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
        assert!(shell.system.ran.iter().all(|c| c.builtin && c.redirections.len() == 1 && matches!(c.redirections[0], Redirection::Out { fd: 1, .. })));
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
        assert!(shell.system.ran[0].builtin && !shell.system.ran[1].builtin);
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
            ("a; if x; then y; fi", "if is not in this shell yet"),
            ("for f in *; do x; done", "for is not in this shell yet"),
            ("while x; do y; done", "while is not in this shell yet"),
            ("case x in esac", "case is not in this shell yet"),
            ("{ a; }", "a group, { ... }, is not in this shell yet"),
            ("(cd x)", "a subshell, ( ... ), is not in this shell yet"),
            ("a &", "running a command in the background, &, is not in this shell yet"),
            ("cat << END", "a here-document, <<, is not in this shell yet"),
            ("echo $((1+2))", "arithmetic, $((...)), is not in this shell yet"),
            ("a >& file", "a redirection to a file with >& is not in this shell yet"),
            ("echo ${X%.c}", "${NAME%pattern} and ${NAME#pattern} is not in this shell yet"),
        ] {
            let mut shell = shell();
            assert_eq!(shell.run(script), 2, "{script}");
            assert_eq!(shell.system.err, format!("sh: {what}\n"), "{script}");
            assert!(shell.system.ran.is_empty() && shell.system.out.is_empty(), "{script}");
        }
        for (script, what) in [
            ("echo 'open", "a ' with no ' after it"),
            ("echo \"open", "a \" with no \" after it"),
            ("a |", "the end, where a command should be"),
            ("| a", "'|' where a command should be"),
            ("a && && b", "'&&' where a command should be"),
            ("a ;; b", "case is not in this shell yet"),
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
}
