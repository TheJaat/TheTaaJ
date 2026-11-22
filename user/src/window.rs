//! Windows, for programs: opening one, drawing in it, hearing about it.
//!
//! ```ignore
//! let mut window = Window::open(b"counter", 240, 120)?;
//! let theme = Theme::new(window.format());
//! window.surface().fill(Rect::new(0, 0, 240, 120), theme.panel);
//! window.damage(Rect::new(0, 0, 240, 120));
//! loop {
//!     let mut buffer = [0u8; MESSAGE];
//!     match window.event(&mut buffer)? {
//!         Event::Pointer { x, y, buttons } => { /* ... */ }
//!         Event::Close => return 0,
//!         _ => {}
//!     }
//! }
//! ```
//!
//! The window's pixels are memory this program created and maps; the
//! window server maps the same memory, read-only, and copies from it to
//! the screen when told which part changed (`damage`). The window's
//! channel carries everything else. Dropping the `Window` closes the
//! channel, and with it the window - as does the program ending, however
//! it ends.

use abi::rights::{READ, TRANSFER};
use gfx::{Format, Rect, Surface};
use gui::protocol::{Event, Open, Request};

use crate::{Error, Handle};

/// Enough for any message the server sends.
pub const MESSAGE: usize = abi::MESSAGE_MAX as usize;

/// How long the server has to answer an OPEN, in ticks.
const OPEN_WAIT: u64 = 300;

/// Why a window could not be opened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowError {
    /// This program was not given "windows" (or there is no screen).
    NoServer,
    /// The size or title cannot be asked for.
    Invalid,
    /// The server said no: why, in its words.
    Refused([u8; 64], usize),
    /// A system call failed on the way.
    System(Error),
}

impl WindowError {
    pub fn describe(&self) -> &str {
        match self {
            WindowError::NoServer => "there is no window server for this program",
            WindowError::Invalid => "a window of that size or title cannot be asked for",
            WindowError::Refused(why, length) => core::str::from_utf8(&why[..*length]).unwrap_or("refused"),
            WindowError::System(_) => "a system call failed",
        }
    }
}

impl From<Error> for WindowError {
    fn from(error: Error) -> Self {
        WindowError::System(error)
    }
}

/// An open window.
pub struct Window {
    channel: Handle,
    memory: Handle,
    pixels: *mut u32,
    width: u32,
    height: u32,
    id: u32,
    format: Format,
}

impl Window {
    /// Opens a window whose content is `width` x `height` pixels, with
    /// `title`; waits for the server's answer.
    pub fn open(title: &[u8], width: u32, height: u32) -> Result<Window, WindowError> {
        let request = Open { width, height, title };
        let mut message = [0u8; 64];
        let length = request.encode(&mut message).ok_or(WindowError::Invalid)?;
        let server = crate::services().ok_or(WindowError::NoServer)?.lookup("windows").map_err(|_| WindowError::NoServer)?;
        let result = Self::open_with(server, &message[..length], &request);
        let _ = crate::close(server);
        result
    }

    fn open_with(server: Handle, message: &[u8], request: &Open) -> Result<Window, WindowError> {
        // The pixels: whole pages, mapped here to draw in.
        let size = request.bytes().div_ceil(4096) * 4096;
        let memory = crate::memory_create(size)?;
        let pixels = match crate::memory_map(memory, true) {
            Ok(pixels) => pixels.cast::<u32>(),
            Err(error) => {
                let _ = crate::close(memory);
                return Err(error.into());
            }
        };
        // From here on, a failure must undo the mapping and the memory:
        // `Window`'s Drop does, so build one now with what there is.
        let mut window = Window { channel: 0, memory, pixels, width: request.width, height: request.height, id: 0, format: Format::XRGB8888 };
        let (ours, theirs) = crate::channel_create()?;
        window.channel = ours;
        // The server gets a copy it can only read.
        let seen = match crate::duplicate(memory, READ | TRANSFER) {
            Ok(seen) => seen,
            Err(error) => {
                let _ = crate::close(theirs);
                return Err(error.into());
            }
        };
        if let Err(error) = crate::send(server, message, &[theirs, seen]) {
            let _ = crate::close(theirs);
            let _ = crate::close(seen);
            return Err(error.into());
        }
        let mut answer = [0u8; MESSAGE];
        let deadline = crate::ticks() + OPEN_WAIT;
        if crate::object_wait_until(&[ours], deadline)?.is_none() {
            return Err(WindowError::System(Error::TimedOut));
        }
        let n = window.receive(&mut answer)?;
        match Event::decode(&answer[..n]) {
            Some(Event::Opened { id, format }) => {
                window.id = id;
                window.format = format;
                Ok(window)
            }
            Some(Event::Refused(why)) => {
                let mut kept = [0u8; 64];
                let length = why.len().min(kept.len());
                kept[..length].copy_from_slice(&why[..length]);
                Err(WindowError::Refused(kept, length))
            }
            _ => Err(WindowError::System(Error::InvalidArgument)),
        }
    }

    /// The window's number, as the server knows it.
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixel format to draw in: the screen's.
    pub fn format(&self) -> Format {
        self.format
    }

    /// The channel, to wait on together with other things.
    pub fn handle(&self) -> Handle {
        self.channel
    }

    /// The window's pixels, to draw on. Nothing shows until `damage`.
    pub fn surface(&mut self) -> Surface<'_> {
        let count = self.width as usize * self.height as usize;
        // SAFETY: the mapping holds at least width x height pixels (made
        // so in `open`), lives as long as `self`, and is written only
        // through this slice while `self` is borrowed mutably. The server
        // only reads it.
        let pixels = unsafe { core::slice::from_raw_parts_mut(self.pixels, count) };
        Surface::new(pixels, self.width, self.height, self.width as usize).expect("exactly width x height pixels")
    }

    /// Tells the server this part changed. If its queue is full, the
    /// damage is lost for now - the next damage, or the next time the
    /// server draws the window, shows it.
    pub fn damage(&self, rect: Rect) {
        let rect = rect.clip(&Rect::new(0, 0, self.width, self.height));
        let mut message = [0u8; 16];
        if let Some(n) = Request::Damage(rect).encode(&mut message) {
            let _ = crate::send(self.channel, &message[..n], &[]);
        }
    }

    pub fn set_title(&self, title: &[u8]) {
        let mut message = [0u8; 64];
        if let Some(n) = Request::Title(title).encode(&mut message) {
            let _ = crate::send(self.channel, &message[..n], &[]);
        }
    }

    /// Waits for the next event about the window. `Err(PeerClosed)`:
    /// the server is gone, and the window with it.
    pub fn event<'b>(&self, buffer: &'b mut [u8; MESSAGE]) -> Result<Event<'b>, Error> {
        loop {
            let n = self.receive(buffer)?;
            // A message this library cannot read is skipped: a newer
            // server may say more than this program knows.
            if Event::decode(&buffer[..n]).is_some() {
                return Event::decode(&buffer[..n]).ok_or(Error::InvalidArgument);
            }
        }
    }

    /// One message, its handles (there should be none) closed.
    fn receive(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let (n, count) = crate::receive(self.channel, buffer, &mut handles)?;
        for &handle in &handles[..count] {
            let _ = crate::close(handle);
        }
        Ok(n)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if self.channel != 0 {
            let _ = crate::close(self.channel);
        }
        let _ = crate::memory_unmap(self.pixels.cast());
        let _ = crate::close(self.memory);
    }
}
