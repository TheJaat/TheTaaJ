//! The scheduler's decisions, separate from the mechanism.
//!
//! Which thread runs next is pure bookkeeping: queues and states. How
//! the CPU actually switches from one thread to another - saving
//! registers, changing stacks - is assembly in the kernel binary. Keeping
//! them apart means every scheduling rule here is tested on the host,
//! where a wrong decision is an assertion failure rather than a hang.
//!
//! The policy is round-robin with time slices:
//!   - runnable threads wait in a FIFO queue
//!   - the running thread gets SLICE timer ticks; when they run out and
//!     someone else is waiting, it goes to the back of the queue
//!   - a thread can also give up the CPU early (yield), sleep until a
//!     tick, block until woken, or exit
//!   - the idle thread runs only when nothing else can. It never sits in
//!     the queue.
//!
//! Simple and fair. Priorities, and a smarter policy for interactive
//! work, come later - behind the same interface.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

pub type ThreadId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Ready,
    /// Until the given tick.
    Sleeping(u64),
    /// Until someone calls `wake`.
    Blocked,
    /// Finished; its resources are freed once it is off the CPU.
    Exited,
}

/// How many ticks a thread may run before others get a turn.
pub const SLICE: u64 = 2;

pub struct Scheduler {
    states: BTreeMap<ThreadId, State>,
    ready: VecDeque<ThreadId>,
    current: ThreadId,
    idle: ThreadId,
    slice_left: u64,
    /// Set when a waiting thread should run as soon as possible - a
    /// wakeup while the CPU idles, or the end of a slice.
    reschedule: bool,
}

/// A switch the caller must carry out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch {
    pub from: ThreadId,
    pub to: ThreadId,
}

impl Scheduler {
    /// A scheduler whose only thread is `idle`, already running.
    pub fn new(idle: ThreadId) -> Scheduler {
        Scheduler::with_running(idle, idle)
    }

    /// A scheduler with `running` on the CPU now (the boot thread) and
    /// `idle` waiting in the wings for when nothing else can run.
    pub fn with_running(idle: ThreadId, running: ThreadId) -> Scheduler {
        let mut states = BTreeMap::new();
        states.insert(idle, State::Ready); // ready, but never queued
        states.insert(running, State::Running);
        Scheduler { states, ready: VecDeque::new(), current: running, idle, slice_left: SLICE, reschedule: false }
    }

    pub fn current(&self) -> ThreadId {
        self.current
    }

    pub fn state(&self, id: ThreadId) -> Option<State> {
        self.states.get(&id).copied()
    }

    pub fn thread_count(&self) -> usize {
        self.states.len()
    }

    /// A new thread, ready to run.
    pub fn add(&mut self, id: ThreadId) {
        assert!(self.states.insert(id, State::Ready).is_none(), "thread {id} added twice");
        self.ready.push_back(id);
        if self.current == self.idle {
            self.reschedule = true;
        }
    }

    /// One timer tick at time `now`. Wakes sleepers whose time has come
    /// and counts down the running thread's slice.
    pub fn tick(&mut self, now: u64) {
        let due: Vec<ThreadId> = self
            .states
            .iter()
            .filter(|(_, s)| matches!(s, State::Sleeping(until) if *until <= now))
            .map(|(&id, _)| id)
            .collect();
        for id in due {
            self.make_ready(id);
        }
        self.slice_left = self.slice_left.saturating_sub(1);
        if self.slice_left == 0 {
            if self.ready.is_empty() {
                // Nobody waiting: a fresh slice, so a thread that turns
                // up later still finds a full slice ahead of it to wait
                // out - not an exhausted one that hands over at once.
                self.slice_left = SLICE;
            } else {
                self.reschedule = true;
            }
        }
    }

    /// Whether a switch is due; clears the request.
    pub fn take_reschedule(&mut self) -> bool {
        core::mem::take(&mut self.reschedule)
    }

    fn make_ready(&mut self, id: ThreadId) {
        self.states.insert(id, State::Ready);
        self.ready.push_back(id);
        if self.current == self.idle {
            self.reschedule = true;
        }
    }

    /// Wakes a blocked or sleeping thread. False if it was neither.
    pub fn wake(&mut self, id: ThreadId) -> bool {
        match self.states.get(&id) {
            Some(State::Blocked | State::Sleeping(_)) => {
                self.make_ready(id);
                true
            }
            _ => false,
        }
    }

    /// Takes the running thread off the CPU at the next `pick_next`,
    /// into `state` (Sleeping, Blocked or Exited).
    pub fn suspend_current(&mut self, state: State) {
        assert!(
            matches!(state, State::Sleeping(_) | State::Blocked | State::Exited),
            "suspend into {state:?}"
        );
        // The idle thread is what runs when nothing else can, so it can
        // never wait for anything: pick_next would simply choose it again.
        assert!(self.current != self.idle, "the idle thread cannot sleep, block or exit");
        self.states.insert(self.current, state);
    }

    /// Chooses who runs next. The current thread, if still running, goes
    /// to the back of the queue (unless it is the idle thread). Returns
    /// the switch to make, or None to keep running the current thread.
    pub fn pick_next(&mut self) -> Option<Switch> {
        let from = self.current;
        let still_running = self.states.get(&from) == Some(&State::Running);

        let Some(to) = self.ready.pop_front() else {
            if still_running {
                self.slice_left = SLICE;
                return None; // nobody else wants the CPU
            }
            return self.switch_to(from, self.idle);
        };
        if still_running && from != self.idle {
            self.states.insert(from, State::Ready);
            self.ready.push_back(from);
        }
        self.switch_to(from, to)
    }

    fn switch_to(&mut self, from: ThreadId, to: ThreadId) -> Option<Switch> {
        self.states.insert(to, State::Running);
        self.current = to;
        self.slice_left = SLICE;
        (from != to).then_some(Switch { from, to })
    }

    /// Forgets an exited thread, once it is off the CPU. Returns whether
    /// it was there to forget.
    pub fn remove(&mut self, id: ThreadId) -> bool {
        if id == self.current || self.states.get(&id) != Some(&State::Exited) {
            return false;
        }
        self.states.remove(&id);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: ThreadId = 0;

    /// Runs `pick_next` and returns who is running afterwards.
    fn next(s: &mut Scheduler) -> ThreadId {
        s.pick_next();
        s.current()
    }

    #[test]
    fn idle_runs_when_nothing_else_can() {
        let mut s = Scheduler::new(IDLE);
        assert_eq!(s.pick_next(), None);
        assert_eq!(s.current(), IDLE);
    }

    #[test]
    fn round_robin_in_order_and_idle_never_queued() {
        let mut s = Scheduler::new(IDLE);
        for id in 1..=3 {
            s.add(id);
        }
        assert!(s.take_reschedule(), "new work while idle asks for a switch");
        let order: Vec<_> = (0..7).map(|_| next(&mut s)).collect();
        assert_eq!(order, vec![1, 2, 3, 1, 2, 3, 1]);
    }

    #[test]
    fn a_slice_runs_out_only_when_someone_is_waiting() {
        let mut s = Scheduler::new(IDLE);
        s.add(1);
        next(&mut s);
        s.take_reschedule();
        for t in 1..=10 {
            s.tick(t);
        }
        assert!(!s.take_reschedule(), "alone: keeps the CPU");

        s.add(2);
        s.tick(11);
        assert!(!s.take_reschedule(), "slice not over yet");
        s.tick(12);
        assert!(s.take_reschedule(), "slice over, 2 is waiting");
        assert_eq!(next(&mut s), 2);
    }

    #[test]
    fn sleepers_wake_on_their_tick() {
        let mut s = Scheduler::new(IDLE);
        s.add(1);
        next(&mut s);
        s.suspend_current(State::Sleeping(5));
        assert_eq!(next(&mut s), IDLE);
        s.take_reschedule();

        s.tick(4);
        assert_eq!(s.state(1), Some(State::Sleeping(5)));
        assert!(!s.take_reschedule());
        s.tick(5);
        assert_eq!(s.state(1), Some(State::Ready));
        assert!(s.take_reschedule(), "the idle CPU should switch at once");
        assert_eq!(next(&mut s), 1);
    }

    #[test]
    fn blocked_threads_are_skipped_until_woken() {
        let mut s = Scheduler::new(IDLE);
        s.add(1);
        s.add(2);
        assert_eq!(next(&mut s), 1);
        s.suspend_current(State::Blocked);
        assert_eq!(next(&mut s), 2);
        assert_eq!(next(&mut s), 2, "1 is blocked; 2 keeps going");

        assert!(s.wake(1));
        assert!(!s.wake(1), "already awake");
        assert_eq!(next(&mut s), 1);
    }

    #[test]
    fn a_thread_that_blocks_and_is_woken_before_switching_keeps_running() {
        // The race a wait queue must survive: block, then the wakeup
        // arrives before the switch happens.
        let mut s = Scheduler::new(IDLE);
        s.add(1);
        next(&mut s);
        s.suspend_current(State::Blocked);
        s.wake(1);
        assert_eq!(next(&mut s), 1);
        assert_eq!(s.state(1), Some(State::Running));
    }

    #[test]
    fn exited_threads_leave_and_are_removed_once_off_the_cpu() {
        let mut s = Scheduler::new(IDLE);
        s.add(1);
        next(&mut s);
        s.suspend_current(State::Exited);
        assert!(!s.remove(1), "still on the CPU");
        assert_eq!(s.pick_next(), Some(Switch { from: 1, to: IDLE }));
        assert!(s.remove(1));
        assert_eq!(s.thread_count(), 1);
        assert!(!s.wake(1), "gone");
    }

    #[test]
    #[should_panic(expected = "idle thread cannot sleep")]
    fn the_idle_thread_cannot_sleep() {
        let mut s = Scheduler::new(IDLE);
        s.suspend_current(State::Sleeping(10));
    }

    #[test]
    fn a_boot_thread_can_sleep_and_exit_leaving_idle() {
        const BOOT: ThreadId = 1;
        let mut s = Scheduler::with_running(IDLE, BOOT);
        s.suspend_current(State::Sleeping(3));
        assert_eq!(s.pick_next(), Some(Switch { from: BOOT, to: IDLE }));
        s.tick(3);
        assert!(s.take_reschedule());
        assert_eq!(next(&mut s), BOOT);
        s.suspend_current(State::Exited);
        assert_eq!(next(&mut s), IDLE);
        assert!(s.remove(BOOT));
        assert_eq!(s.thread_count(), 1);
    }

    /// Many threads, random operations: every thread that is ready is
    /// eventually run, nobody runs twice in a row while others wait, and
    /// the idle thread is never queued.
    #[test]
    fn randomized_fairness() {
        let mut s = Scheduler::new(IDLE);
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for id in 1..=20 {
            s.add(id);
        }
        let mut now = 0;
        let mut ran = BTreeMap::new();
        for _ in 0..10_000 {
            now += 1;
            s.tick(now);
            match random() % 10 {
                0 if s.current() != IDLE => s.suspend_current(State::Sleeping(now + random() % 5)),
                1 if s.current() != IDLE => s.suspend_current(State::Blocked),
                2 => {
                    let id = 1 + random() % 20;
                    s.wake(id);
                }
                _ => {}
            }
            let before = s.current();
            s.pick_next();
            let after = s.current();
            *ran.entry(after).or_insert(0u32) += 1;
            assert!(!s.ready.contains(&IDLE));
            if before == after && after != IDLE {
                assert!(s.ready.is_empty(), "kept the CPU while others waited");
            }
        }
        for id in 1..=20 {
            assert!(ran.get(&id).copied().unwrap_or(0) > 50, "thread {id} starved");
        }
    }
}
