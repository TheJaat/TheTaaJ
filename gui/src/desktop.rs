//! The window server's bookkeeping: where each window is, which is on
//! top, which has the keys, and what the pointer is doing to them.
//!
//! No pixels here, and no channels: the server tells `Desktop` what the
//! pointer did, and gets back a `Response` - which parts of the screen
//! must be drawn again, which window the click is for, whether the focus
//! moved, whether a drag ended, whether a close box was clicked. So the
//! whole of a window manager's behaviour is tested on the host.
//!
//! A window on the screen is its *content* - the rectangle its program
//! draws - inside a *frame* the server draws: a border, and a title bar
//! above the content with the title and, if the window may be closed, a
//! close box.
//!
//! ```text
//!   frame ┌──────────────────────────────┐
//!         │ title                    [x] │  title bar (TITLE_HEIGHT)
//!         ├──────────────────────────────┤
//!         │                              │
//!         │          content             │  the program's pixels
//!         │                              │
//!         └──────────────────────────────┘  border (BORDER) all round
//! ```
//!
//! The windows are kept in stacking order, bottom first. Pressing a
//! button on a window raises it and gives it the focus. Then:
//!
//!   - on its title bar: the window follows the pointer until the button
//!     is let go (a drag);
//!   - on its close box: when the button is let go over the same box,
//!     the program is asked to close the window;
//!   - on its content: the press goes to the program, and so does
//!     everything the pointer does until every button is up (a grab -
//!     a drag that leaves the window still belongs to it).

use gfx::Rect;

use crate::protocol::TITLE_MAX;

/// The most windows at once.
pub const WINDOWS_MAX: usize = 8;
/// The frame's border, all round, in pixels.
pub const BORDER: u32 = 1;
/// The title bar's height.
pub const TITLE_HEIGHT: u32 = 22;
/// The close box: a square, this wide, near the title bar's right end.
pub const CLOSE_SIZE: u32 = 14;
/// How much of a title bar must stay on the screen, across: a window
/// may be dragged mostly out of sight, never out of reach.
pub const KEEP_VISIBLE: u32 = 48;
/// Where the first window a program opens is placed (its frame's
/// top-left corner), and how far down and right each next one goes.
pub const CASCADE: (i32, i32) = (200, 150);
pub const CASCADE_STEP: i32 = 32;

/// The left button: the one that raises, drags and closes.
pub const LEFT: u8 = 1;

/// A window's number. 0 is never one.
pub type Id = u32;

/// A window, as the server keeps it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Window {
    pub id: Id,
    /// Where the program's pixels go on the screen, and their size.
    pub content: Rect,
    /// Whether it has a close box.
    pub closable: bool,
    title: [u8; TITLE_MAX],
    title_length: usize,
}

/// The parts of a window the pointer can be on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    Title,
    Close,
    Content,
    Border,
}

impl Window {
    pub fn title(&self) -> &[u8] {
        &self.title[..self.title_length]
    }

    /// Everything the window covers: content, title bar and border.
    pub fn frame(&self) -> Rect {
        let b = BORDER as i32;
        Rect::new(
            self.content.x - b,
            self.content.y - b - TITLE_HEIGHT as i32,
            self.content.width + 2 * BORDER,
            self.content.height + TITLE_HEIGHT + 2 * BORDER,
        )
    }

    /// The title bar: as wide as the content, above it.
    pub fn title_bar(&self) -> Rect {
        Rect::new(self.content.x, self.content.y - TITLE_HEIGHT as i32, self.content.width, TITLE_HEIGHT)
    }

    /// The close box, if it has one: centred in the title bar's height,
    /// as far from the right end as from the top.
    pub fn close_box(&self) -> Option<Rect> {
        let bar = self.title_bar();
        let inset = (TITLE_HEIGHT - CLOSE_SIZE) / 2;
        // A window too narrow for the box and a gap has none.
        (self.closable && bar.width >= CLOSE_SIZE + 2 * inset).then(|| {
            Rect::new(bar.x + (bar.width - CLOSE_SIZE - inset) as i32, bar.y + inset as i32, CLOSE_SIZE, CLOSE_SIZE)
        })
    }

    /// Which part of the window (x, y) is on, if any.
    pub fn part(&self, x: i32, y: i32) -> Option<Part> {
        if self.close_box().is_some_and(|close| close.contains(x, y)) {
            Some(Part::Close)
        } else if self.title_bar().contains(x, y) {
            Some(Part::Title)
        } else if self.content.contains(x, y) {
            Some(Part::Content)
        } else if self.frame().contains(x, y) {
            Some(Part::Border)
        } else {
            None
        }
    }

    fn set_title(&mut self, title: &[u8]) {
        let length = title.len().min(TITLE_MAX);
        self.title[..length].copy_from_slice(&title[..length]);
        self.title_length = length;
    }
}

/// Up to four rectangles of the screen to draw again. More are merged
/// into one that holds them all: drawing a little too much is right,
/// drawing too little is not.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Damage {
    rects: [Rect; 4],
    count: usize,
}

impl Damage {
    pub fn add(&mut self, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        if self.count == self.rects.len() {
            let all = self.rects.iter().fold(Rect::default(), |all, r| all.union(r));
            self.rects[0] = all;
            self.count = 1;
        }
        self.rects[self.count] = rect;
        self.count += 1;
    }

    pub fn rects(&self) -> &[Rect] {
        &self.rects[..self.count]
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// What the server must do after a pointer report.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Response {
    /// Parts of the screen to draw again.
    pub damage: Damage,
    /// The report is for this window's program: where, in its content's
    /// coordinates, and the buttons.
    pub deliver: Option<(Id, i32, i32, u8)>,
    /// The focus moved: from (None if no window had it), to.
    pub focus: Option<(Option<Id>, Id)>,
    /// A drag ended: the window, and its frame's top-left corner now.
    pub moved: Option<(Id, i32, i32)>,
    /// This window's close box was clicked.
    pub close: Option<Id>,
}

/// A drag of a window by its title bar: which, and where the pointer
/// is in its frame (kept the same as it moves).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Drag {
    id: Id,
    dx: i32,
    dy: i32,
}

/// Why a window could not be opened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// WINDOWS_MAX are open.
    Full,
    /// Its frame would be bigger than the screen.
    TooBig,
}

impl Refusal {
    pub fn describe(self) -> &'static str {
        match self {
            Refusal::Full => "no room for another window",
            Refusal::TooBig => "bigger than the screen",
        }
    }
}

/// The screen's windows.
pub struct Desktop {
    screen: Rect,
    /// windows[..count], bottom first.
    windows: [Option<Window>; WINDOWS_MAX],
    count: usize,
    focus: Option<Id>,
    next_id: Id,
    /// How many windows have been placed by cascade: the next goes one
    /// step further.
    cascaded: i32,
    buttons: u8,
    drag: Option<Drag>,
    grab: Option<Id>,
    /// The close box the left button went down on.
    closing: Option<Id>,
}

impl Desktop {
    pub fn new(width: u32, height: u32) -> Desktop {
        Desktop {
            screen: Rect::new(0, 0, width, height),
            windows: [None; WINDOWS_MAX],
            count: 0,
            focus: None,
            next_id: 1,
            cascaded: 0,
            buttons: 0,
            drag: None,
            grab: None,
            closing: None,
        }
    }

    /// Opens a window with content `width` x `height`. Its frame's
    /// top-left corner at `at`, or - None - where the cascade puts it.
    /// It goes on top and gets the focus; the Response says what to
    /// draw and that the focus moved.
    pub fn open(&mut self, title: &[u8], width: u32, height: u32, closable: bool, at: Option<(i32, i32)>) -> Result<(Id, Response), Refusal> {
        if self.count == WINDOWS_MAX {
            return Err(Refusal::Full);
        }
        let (frame_width, frame_height) = (width + 2 * BORDER, height + TITLE_HEIGHT + 2 * BORDER);
        if width == 0 || height == 0 || frame_width > self.screen.width || frame_height > self.screen.height {
            return Err(Refusal::TooBig);
        }
        let (x, y) = at.unwrap_or_else(|| {
            let step = (self.cascaded % 8) * CASCADE_STEP;
            self.cascaded += 1;
            // Pushed back in if the step would take it off the screen.
            let x = (CASCADE.0 + step).min((self.screen.width - frame_width) as i32);
            let y = (CASCADE.1 + step).min((self.screen.height - frame_height) as i32);
            (x, y)
        });
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        let b = BORDER as i32;
        let mut window = Window {
            id,
            content: Rect::new(x + b, y + b + TITLE_HEIGHT as i32, width, height),
            closable,
            title: [0; TITLE_MAX],
            title_length: 0,
        };
        window.set_title(title);
        self.windows[self.count] = Some(window);
        self.count += 1;
        let mut response = Response::default();
        response.damage.add(window.frame());
        self.give_focus(id, &mut response);
        Ok((id, response))
    }

    /// Closes a window: what was under it must be drawn, and if it had
    /// the focus, the window now on top gets it.
    pub fn close(&mut self, id: Id) -> Response {
        let mut response = Response::default();
        let Some(index) = self.index(id) else { return response };
        let window = self.windows[index].expect("inside the count");
        self.windows.copy_within(index + 1..self.count, index);
        self.count -= 1;
        self.windows[self.count] = None;
        response.damage.add(window.frame());
        if self.drag.is_some_and(|drag| drag.id == id) {
            self.drag = None;
        }
        if self.grab == Some(id) {
            self.grab = None;
        }
        if self.closing == Some(id) {
            self.closing = None;
        }
        if self.focus == Some(id) {
            self.focus = None;
            if let Some(top) = self.stack().last() {
                let top = top.id;
                self.give_focus(top, &mut response);
            }
        }
        response
    }

    /// A new title: the title bar to draw again, if the window exists.
    pub fn set_title(&mut self, id: Id, title: &[u8]) -> Option<Rect> {
        let index = self.index(id)?;
        let window = self.windows[index].as_mut().expect("inside the count");
        window.set_title(title);
        Some(window.title_bar())
    }

    pub fn window(&self, id: Id) -> Option<&Window> {
        self.index(id).and_then(|index| self.windows[index].as_ref())
    }

    /// The windows, bottom first - the order to draw them in.
    pub fn stack(&self) -> impl DoubleEndedIterator<Item = &Window> {
        self.windows[..self.count].iter().flatten()
    }

    pub fn focus(&self) -> Option<Id> {
        self.focus
    }

    pub fn screen(&self) -> Rect {
        self.screen
    }

    /// The top window under (x, y), and which part of it.
    pub fn at(&self, x: i32, y: i32) -> Option<(Id, Part)> {
        self.stack().rev().find_map(|window| window.part(x, y).map(|part| (window.id, part)))
    }

    /// The pointer is at (x, y), with `buttons` held.
    pub fn pointer(&mut self, x: i32, y: i32, buttons: u8) -> Response {
        let mut response = Response::default();
        let pressed = buttons & !self.buttons;
        let released = self.buttons & !buttons;
        self.buttons = buttons;

        if let Some(drag) = self.drag {
            if buttons & LEFT != 0 {
                self.move_window(drag.id, x - drag.dx, y - drag.dy, &mut response);
            } else {
                self.drag = None;
                if let Some(window) = self.window(drag.id) {
                    let frame = window.frame();
                    response.moved = Some((drag.id, frame.x, frame.y));
                }
            }
            return response;
        }
        if let Some(id) = self.grab {
            if let Some(window) = self.window(id) {
                response.deliver = Some((id, x - window.content.x, y - window.content.y, buttons));
            }
            if buttons == 0 {
                self.grab = None;
            }
            return response;
        }
        if let Some(id) = self.closing {
            if released & LEFT != 0 {
                self.closing = None;
                if self.at(x, y) == Some((id, Part::Close)) {
                    response.close = Some(id);
                }
            }
            return response;
        }
        if pressed == 0 {
            return response;
        }
        let Some((id, part)) = self.at(x, y) else { return response };
        self.raise(id, &mut response);
        self.give_focus(id, &mut response);
        let frame = self.window(id).expect("just found").frame();
        match part {
            Part::Title if pressed & LEFT != 0 => self.drag = Some(Drag { id, dx: x - frame.x, dy: y - frame.y }),
            Part::Close if pressed & LEFT != 0 => self.closing = Some(id),
            Part::Content => {
                let content = self.window(id).expect("just found").content;
                response.deliver = Some((id, x - content.x, y - content.y, buttons));
                self.grab = Some(id);
            }
            _ => {}
        }
        response
    }

    fn index(&self, id: Id) -> Option<usize> {
        self.windows[..self.count].iter().position(|window| window.is_some_and(|w| w.id == id))
    }

    /// Puts a window on top: all of it is drawn again (what covered it
    /// is now under it).
    fn raise(&mut self, id: Id, response: &mut Response) {
        let Some(index) = self.index(id) else { return };
        if index + 1 == self.count {
            return;
        }
        let window = self.windows[index];
        self.windows.copy_within(index + 1..self.count, index);
        self.windows[self.count - 1] = window;
        if let Some(window) = window {
            response.damage.add(window.frame());
        }
    }

    /// Gives a window the focus: both title bars change colour.
    fn give_focus(&mut self, id: Id, response: &mut Response) {
        if self.focus == Some(id) {
            return;
        }
        let from = self.focus;
        if let Some(old) = from.and_then(|old| self.window(old)) {
            response.damage.add(old.title_bar());
        }
        if let Some(new) = self.window(id) {
            response.damage.add(new.title_bar());
        }
        self.focus = Some(id);
        response.focus = Some((from, id));
    }

    /// Moves a window's frame to (x, y) - kept where its title bar can
    /// still be reached - drawing where it was and where it is.
    fn move_window(&mut self, id: Id, x: i32, y: i32, response: &mut Response) {
        let screen = self.screen;
        let Some(index) = self.index(id) else { return };
        let window = self.windows[index].as_mut().expect("inside the count");
        let frame = window.frame();
        let keep = KEEP_VISIBLE.min(frame.width) as i32;
        let x = x.clamp(keep - frame.width as i32, screen.width as i32 - keep);
        let y = y.clamp(0, screen.height as i32 - (TITLE_HEIGHT + BORDER) as i32);
        let (dx, dy) = (x - frame.x, y - frame.y);
        if (dx, dy) == (0, 0) {
            return;
        }
        window.content = window.content.offset(dx, dy);
        response.damage.add(frame);
        response.damage.add(window.frame());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop() -> Desktop {
        Desktop::new(1024, 768)
    }

    /// A press and a release of the left button at (x, y).
    fn click(desktop: &mut Desktop, x: i32, y: i32) -> (Response, Response) {
        (desktop.pointer(x, y, LEFT), desktop.pointer(x, y, 0))
    }

    #[test]
    fn a_window_is_its_content_in_a_frame() {
        let mut d = desktop();
        let (id, response) = d.open(b"counter", 200, 100, true, Some((100, 50))).unwrap();
        let w = *d.window(id).unwrap();
        assert_eq!(w.frame(), Rect::new(100, 50, 202, 124));
        assert_eq!(w.content, Rect::new(101, 73, 200, 100));
        assert_eq!(w.title_bar(), Rect::new(101, 51, 200, 22));
        assert_eq!(w.close_box(), Some(Rect::new(101 + 200 - 14 - 4, 55, 14, 14)));
        assert_eq!(w.title(), b"counter");
        // Opening draws it, and gives it the focus.
        assert_eq!(response.damage.rects()[0], w.frame());
        assert_eq!(response.focus, Some((None, id)));
        assert_eq!(d.focus(), Some(id));
        // The parts.
        assert_eq!(w.part(105, 60), Some(Part::Title));
        assert_eq!(w.part(290, 60), Some(Part::Close));
        assert_eq!(w.part(150, 100), Some(Part::Content));
        assert_eq!(w.part(100, 100), Some(Part::Border));
        assert_eq!(w.part(99, 100), None);
        // One that cannot be closed has no close box: that corner is title.
        let (other, _) = d.open(b"terminal", 200, 100, false, Some((500, 50))).unwrap();
        let t = d.window(other).unwrap();
        assert_eq!(t.close_box(), None);
        assert_eq!(t.part(690, 60), Some(Part::Title));
        // Too narrow for a box: none.
        let (narrow, _) = d.open(b"", 10, 10, true, Some((0, 300))).unwrap();
        assert_eq!(d.window(narrow).unwrap().close_box(), None);
    }

    #[test]
    fn windows_cascade_and_stay_on_the_screen() {
        let mut d = desktop();
        let (a, _) = d.open(b"a", 100, 50, true, None).unwrap();
        let (b, _) = d.open(b"b", 100, 50, true, None).unwrap();
        assert_eq!((d.window(a).unwrap().frame().x, d.window(a).unwrap().frame().y), CASCADE);
        assert_eq!(d.window(b).unwrap().frame().x, CASCADE.0 + CASCADE_STEP);
        // A big one is pushed back so it fits.
        let (big, _) = d.open(b"big", 1000, 700, true, None).unwrap();
        assert_eq!(d.window(big).unwrap().frame(), Rect::new(1024 - 1002, 768 - 724, 1002, 724));
        // Bigger than the screen, or nothing at all: refused.
        assert_eq!(d.open(b"huge", 1023, 100, true, None).err(), Some(Refusal::TooBig));
        assert_eq!(d.open(b"none", 0, 100, true, None).err(), Some(Refusal::TooBig));
    }

    #[test]
    fn there_is_room_for_so_many_windows() {
        let mut d = desktop();
        for n in 0..WINDOWS_MAX {
            assert!(d.open(b"w", 10, 10, true, None).is_ok(), "window {n}");
        }
        assert_eq!(d.open(b"one more", 10, 10, true, None).err(), Some(Refusal::Full));
        // Closing one makes room again.
        let top = d.stack().last().unwrap().id;
        d.close(top);
        assert!(d.open(b"again", 10, 10, true, None).is_ok());
    }

    #[test]
    fn a_click_raises_focuses_and_goes_to_the_program() {
        let mut d = desktop();
        let (back, _) = d.open(b"back", 300, 200, true, Some((100, 100))).unwrap();
        let (front, _) = d.open(b"front", 300, 200, true, Some((200, 150))).unwrap();
        assert_eq!(d.focus(), Some(front));
        // Where they overlap, the front one is hit.
        assert_eq!(d.at(250, 250), Some((front, Part::Content)));
        // A click on the part of `back` that shows.
        let (down, up) = click(&mut d, 120, 200);
        let content = d.window(back).unwrap().content;
        assert_eq!(down.deliver, Some((back, 120 - content.x, 200 - content.y, LEFT)));
        assert_eq!(down.focus, Some((Some(front), back)));
        assert_eq!(d.stack().last().unwrap().id, back);
        // All of it drawn again (it is on top now), and both title bars.
        assert!(down.damage.rects().contains(&d.window(back).unwrap().frame()));
        assert!(down.damage.rects().contains(&d.window(front).unwrap().title_bar()));
        // The release goes to it too.
        assert_eq!(up.deliver, Some((back, 120 - content.x, 200 - content.y, 0)));
        assert_eq!(up.focus, None);
        // Now `back` is hit where they overlap.
        assert_eq!(d.at(250, 250), Some((back, Part::Content)));
        // A click on the desktop: nothing, and the focus stays.
        let (down, _) = click(&mut d, 900, 700);
        assert_eq!(down, Response::default());
        assert_eq!(d.focus(), Some(back));
        // Moving with no button held tells no one.
        assert_eq!(d.pointer(150, 250, 0), Response::default());
    }

    #[test]
    fn a_held_button_keeps_going_to_the_window_it_went_down_in() {
        let mut d = desktop();
        let (id, _) = d.open(b"w", 100, 100, true, Some((100, 100))).unwrap();
        let (other, _) = d.open(b"other", 100, 100, true, Some((400, 100))).unwrap();
        let content = d.window(id).unwrap().content;
        d.pointer(content.x + 5, content.y + 5, LEFT);
        // Out of the window and over the other one: still this one's,
        // and the other does not get the focus.
        let r = d.pointer(450, 150, LEFT);
        assert_eq!(r.deliver, Some((id, 450 - content.x, 150 - content.y, LEFT)));
        assert_eq!(d.focus(), Some(id));
        // A second button, then both let go: all to it.
        assert_eq!(d.pointer(450, 150, LEFT | 2).deliver.map(|d| d.3), Some(LEFT | 2));
        assert_eq!(d.pointer(450, 150, 0).deliver.map(|d| d.0), Some(id));
        // The grab is over: the next press goes to the window under it.
        assert_eq!(d.pointer(450, 150, LEFT).deliver.map(|d| d.0), Some(other));
    }

    #[test]
    fn a_title_bar_drags_its_window() {
        let mut d = desktop();
        let (id, _) = d.open(b"w", 200, 100, true, Some((100, 100))).unwrap();
        let before = d.window(id).unwrap().frame();
        // Down on the title, 10 right and 5 down from the frame's corner.
        let down = d.pointer(110, 105, LEFT);
        assert_eq!(down.deliver, None);
        let r = d.pointer(310, 205, LEFT);
        let after = d.window(id).unwrap().frame();
        assert_eq!((after.x, after.y), (300, 200));
        // Both where it was and where it is are drawn.
        assert_eq!(r.damage.rects(), &[before, after]);
        assert_eq!(r.moved, None);
        // Let go: the drag ends, and says where.
        let up = d.pointer(310, 205, 0);
        assert_eq!(up.moved, Some((id, 300, 200)));
        assert!(up.damage.is_empty());
        // The program heard nothing of it.
        assert_eq!(up.deliver, None);
        // Not moving does not draw.
        d.pointer(310, 205, LEFT);
        assert!(d.pointer(310, 205, LEFT).damage.is_empty());
        d.pointer(310, 205, 0);
    }

    #[test]
    fn a_dragged_window_keeps_its_title_bar_within_reach() {
        let mut d = desktop();
        let (id, _) = d.open(b"w", 200, 100, true, Some((100, 100))).unwrap();
        d.pointer(105, 105, LEFT);
        d.pointer(-5000, -5000, LEFT);
        let frame = d.window(id).unwrap().frame();
        assert_eq!((frame.x, frame.y), (KEEP_VISIBLE as i32 - 202, 0));
        d.pointer(5000, 5000, LEFT);
        let frame = d.window(id).unwrap().frame();
        assert_eq!((frame.x, frame.y), (1024 - KEEP_VISIBLE as i32, 768 - 23));
        d.pointer(5000, 5000, 0);
        // Its title bar can still be grabbed, and the window brought back.
        assert_eq!(d.at(1024 - 40, 768 - 10).map(|a| a.0), Some(id));
    }

    #[test]
    fn the_close_box_asks_only_when_let_go_over_it() {
        let mut d = desktop();
        let (id, _) = d.open(b"w", 200, 100, true, Some((100, 100))).unwrap();
        let close = d.window(id).unwrap().close_box().unwrap();
        let (cx, cy) = (close.x + 3, close.y + 3);
        let (down, up) = click(&mut d, cx, cy);
        assert_eq!(down.close, None);
        assert_eq!(up.close, Some(id));
        // The server has not closed it: the program decides.
        assert!(d.window(id).is_some());
        // Down on the box, moved off, let go: changed one's mind.
        d.pointer(cx, cy, LEFT);
        assert_eq!(d.pointer(cx - 50, cy, 0).close, None);
        // Down elsewhere, let go on the box: not a click on it.
        d.pointer(cx - 50, cy, LEFT); // starts a drag, in fact
        assert_eq!(d.pointer(cx, cy, 0).close, None);
    }

    #[test]
    fn closing_gives_the_focus_to_the_window_on_top() {
        let mut d = desktop();
        let (a, _) = d.open(b"a", 100, 100, true, Some((0, 100))).unwrap();
        let (b, _) = d.open(b"b", 100, 100, true, Some((300, 100))).unwrap();
        let (c, _) = d.open(b"c", 100, 100, true, Some((600, 100))).unwrap();
        // Closing one that has no focus moves nothing.
        let r = d.close(a);
        assert_eq!(r.focus, None);
        assert_eq!(r.damage.rects(), &[Rect::new(0, 100, 102, 124)]);
        // Closing the focused one: the one now on top gets it.
        let r = d.close(c);
        assert_eq!(r.focus, Some((None, b)));
        assert_eq!(d.focus(), Some(b));
        // The last: no one has the focus.
        let r = d.close(b);
        assert_eq!((r.focus, d.focus()), (None, None));
        // A window that is not there: nothing.
        assert_eq!(d.close(99), Response::default());
    }

    #[test]
    fn closing_a_window_ends_whatever_the_pointer_was_doing_to_it() {
        let mut d = desktop();
        let (id, _) = d.open(b"w", 200, 100, true, Some((100, 100))).unwrap();
        d.pointer(110, 105, LEFT); // dragging it
        d.close(id);
        // The drag is over: moving does nothing, and does not panic.
        assert_eq!(d.pointer(300, 300, LEFT), Response::default());
        d.pointer(300, 300, 0);
        // And a grab.
        let (id, _) = d.open(b"w", 200, 100, true, Some((100, 100))).unwrap();
        d.pointer(150, 150, LEFT);
        d.close(id);
        assert_eq!(d.pointer(160, 160, LEFT).deliver, None);
    }

    #[test]
    fn a_new_title_is_kept_and_its_bar_drawn() {
        let mut d = desktop();
        let (id, _) = d.open(b"counter", 200, 100, true, Some((100, 100))).unwrap();
        assert_eq!(d.set_title(id, b"counter: 3"), Some(d.window(id).unwrap().title_bar()));
        assert_eq!(d.window(id).unwrap().title(), b"counter: 3");
        // Too long: cut to the longest.
        d.set_title(id, &[b'x'; TITLE_MAX + 5]);
        assert_eq!(d.window(id).unwrap().title().len(), TITLE_MAX);
        assert_eq!(d.set_title(99, b"nobody"), None);
    }

    #[test]
    fn damage_beyond_four_rectangles_becomes_one_that_holds_them() {
        let mut damage = Damage::default();
        damage.add(Rect::default()); // empty: nothing
        assert!(damage.is_empty());
        for n in 0..4 {
            damage.add(Rect::new(n * 10, 0, 5, 5));
        }
        assert_eq!(damage.rects().len(), 4);
        damage.add(Rect::new(100, 100, 1, 1));
        assert_eq!(damage.rects(), &[Rect::new(0, 0, 35, 5), Rect::new(100, 100, 1, 1)]);
    }
}
