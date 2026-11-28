//! counter - a window with a count and two buttons: the first program
//! with a window of its own.
//!
//! `+1` (or the `+` key, typed while the window has the focus) adds one;
//! `quit`, or the close box, ends it. Every key it is given is logged,
//! so a test can tell that keys reach the window with the focus and
//! nowhere else.
//!
//! What it says goes to the kernel's log, not to standard output: its
//! standard output is the terminal - a window like this one, which the
//! test does not read.

#![no_std]
#![no_main]

use core::fmt::Write;

use gfx::Rect;
use gui::protocol::Event;
use gui::toolkit::{Button, Label, Press, Theme};
use taaj::window::{Window, MESSAGE};
use taaj::{eprintln, Buffer};

taaj::program!(main);

const WIDTH: u32 = 240;
const HEIGHT: u32 = 110;

/// A line for the log.
macro_rules! log {
    ($($arg:tt)*) => {{
        let mut line = Buffer::<128>::new();
        let _ = write!(line, "[counter] ");
        let _ = writeln!(line, $($arg)*);
        let _ = taaj::debug_write(line.as_bytes());
    }};
}

struct Counter {
    window: Window,
    theme: Theme,
    count: u32,
    label: Label,
    add: Button,
    quit: Button,
}

impl Counter {
    fn draw_label(&mut self) {
        let mut text = Buffer::<32>::new();
        let _ = write!(text, "clicks: {}", self.count);
        let label = self.label;
        let theme = self.theme;
        label.draw(&mut self.window.surface(), text.as_bytes(), &theme);
        self.window.damage(label.rect);
    }

    fn draw_button(&mut self, which: Which) {
        let button = match which {
            Which::Add => self.add,
            Which::Quit => self.quit,
        };
        let theme = self.theme;
        button.draw(&mut self.window.surface(), &theme);
        self.window.damage(button.rect);
    }

    fn add_one(&mut self) {
        self.count += 1;
        log!("clicks: {}", self.count);
        self.draw_label();
    }
}

#[derive(Clone, Copy)]
enum Which {
    Add,
    Quit,
}

fn main() -> i64 {
    let window = match Window::open(b"counter", WIDTH, HEIGHT) {
        Ok(window) => window,
        Err(error) => {
            eprintln!("counter: no window: {}", error.describe());
            return 1;
        }
    };
    let theme = Theme::new(window.format());
    let mut counter = Counter {
        window,
        theme,
        count: 0,
        label: Label { rect: Rect::new(16, 14, WIDTH - 32, 24) },
        add: Button::new(Rect::new(16, 56, 96, 36), b"+1"),
        quit: Button::new(Rect::new(128, 56, 96, 36), b"quit"),
    };
    counter.window.surface().fill(Rect::new(0, 0, WIDTH, HEIGHT), theme.panel);
    counter.window.damage(Rect::new(0, 0, WIDTH, HEIGHT));
    counter.draw_label();
    counter.draw_button(Which::Add);
    counter.draw_button(Which::Quit);
    log!("window {} open, {}x{}", counter.window.id(), WIDTH, HEIGHT);

    let mut buffer = [0u8; MESSAGE];
    loop {
        let event = match counter.window.event(&mut buffer) {
            Ok(event) => event,
            Err(_) => {
                log!("the window server is gone");
                return 2;
            }
        };
        match event {
            Event::Pointer { x, y, buttons } => {
                for which in [Which::Add, Which::Quit] {
                    let button = match which {
                        Which::Add => &mut counter.add,
                        Which::Quit => &mut counter.quit,
                    };
                    match button.pointer(x, y, buttons) {
                        Press::None => {}
                        Press::Changed => counter.draw_button(which),
                        Press::Clicked => {
                            counter.draw_button(which);
                            match which {
                                Which::Add => counter.add_one(),
                                Which::Quit => {
                                    log!("quit");
                                    return 0;
                                }
                            }
                        }
                    }
                }
            }
            Event::Key(bytes) => {
                for &byte in bytes {
                    if byte.is_ascii_graphic() {
                        log!("key {}", byte as char);
                    } else {
                        log!("key 0x{byte:02x}");
                    }
                    if byte == b'+' {
                        counter.add_one();
                    }
                }
            }
            Event::Focus(focused) => log!("{}", if focused { "has the focus" } else { "lost the focus" }),
            Event::Wheel { x, y, clicks } => log!("wheel {clicks} at ({x}, {y})"),
            Event::KeyCode { code, down, modifiers } => {
                log!("key code {code} {} (modifiers {modifiers})", if down { "down" } else { "up" });
            }
            Event::Close => {
                log!("closed by its close box");
                return 0;
            }
            Event::Opened { .. } | Event::Refused(_) => {}
        }
    }
}
