//! Smooth wheel scrolling, done here rather than by Slint.
//!
//! A precision touchpad's coasting reaches the window as one large wheel
//! step every 40 to 250 ms, and the first comes 150 to 350 ms after the
//! finger leaves the pad, carrying the distance owed for that wait. Slint
//! glides each step out over 180 ms, slowing to a stop, so the list stopped
//! and lurched forward several times a second. Here every step is spread
//! out in time instead, and when one runs out soon after fast scrolling,
//! the list keeps going, easing off, on credit: the late step pays it back.

use slint::platform::WindowEvent;
use slint::winit_030::winit::event::{MouseScrollDelta, WindowEvent as WinitEvent};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, LogicalPosition, Timer, TimerMode};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Logical pixels per wheel line, as Slint itself scrolls them.
const LINE: f32 = 60.0;
/// How long a step glides when nothing better is known: a single notch of
/// a mouse wheel, or each step of the quick stream while a finger is on
/// the touchpad. About what Slint's own glide feels like.
const GLIDE: Duration = Duration::from_millis(90);
/// A step that comes this long after the one before it is taken for
/// touchpad coasting, and glides until the next one is due.
const SPACED: std::ops::RangeInclusive<Duration> =
    Duration::from_millis(50)..=Duration::from_millis(250);
/// How long a big step glides: more than a notch at once comes from a
/// quick flick, and the first coasting step follows the flick's last
/// about this long after. Gliding until then keeps the list moving.
const COAST: Duration = Duration::from_millis(190);
/// How often the list is moved while a glide lasts.
const TICK: Duration = Duration::from_millis(8);
/// The list keeps going on credit only this soon after the last step...
const BRIDGE: Duration = Duration::from_millis(350);
/// ...only when it was going at least this fast, in logical pixels a
/// second (a notch of a mouse wheel glides at about 700)...
const BRIDGE_SPEED: f32 = 2000.0;
/// ...and slowing down with this time constant, in seconds.
const BRIDGE_EASE: f32 = 0.4;

/// The scrolling still to do, and how fast to do it.
#[derive(Default)]
struct Pacer {
    /// Still to scroll, per axis. Past zero, against `dir`, it is credit:
    /// scrolled ahead of the steps, for the next one to pay back.
    pending: [f32; 2],
    /// Which way each axis last went: 1, -1, or 0 before any step.
    dir: [f32; 2],
    // Logical pixels a second, per axis.
    rate: [f32; 2],
    last: Option<Instant>,
}

impl Pacer {
    /// A wheel step came in, in logical pixels.
    fn push(&mut self, now: Instant, delta: [f32; 2]) {
        let gap = self.last.map(|last| now.saturating_duration_since(last));
        self.last = Some(now);
        let big = delta.iter().any(|d| d.abs() > LINE);
        let glide = match gap.filter(|g| SPACED.contains(g)) {
            Some(gap) => gap,
            None if big => COAST,
            None => GLIDE,
        };
        for axis in 0..2 {
            if delta[axis] != 0.0 {
                // Turning around drops what was still to come that way,
                // and any credit.
                if delta[axis] * self.dir[axis] < 0.0 {
                    self.pending[axis] = 0.0;
                }
                self.dir[axis] = delta[axis].signum();
                self.pending[axis] += delta[axis];
            }
            // A step smaller than the credit leaves the rest unpaid.
            if self.pending[axis] * self.dir[axis] <= 0.0 {
                self.pending[axis] = 0.0;
            }
            self.rate[axis] = self.pending[axis] / glide.as_secs_f32();
        }
    }

    /// The scrolling to do at `now`, `elapsed` after the last time.
    fn take(&mut self, now: Instant, elapsed: Duration) -> [f32; 2] {
        let since = self
            .last
            .map_or(Duration::MAX, |last| now.saturating_duration_since(last));
        let dt = elapsed.as_secs_f32();
        let mut out = [0.0; 2];
        for axis in 0..2 {
            let want = self.rate[axis] * dt;
            let left = self.pending[axis] * self.dir[axis];
            if left > 0.0 && want.abs() < left {
                out[axis] = want;
            } else if since < BRIDGE && self.rate[axis].abs() >= BRIDGE_SPEED {
                // The glide ran out soon after fast scrolling: a coasting
                // step is on its way. Keep going, easing off, on credit.
                out[axis] = want;
                self.rate[axis] *= (-dt / BRIDGE_EASE).exp();
            } else {
                // Done. Credit the steps never paid back is let go.
                out[axis] = left.max(0.0) * self.dir[axis];
                self.pending[axis] = out[axis];
                self.rate[axis] = 0.0;
            }
            self.pending[axis] -= out[axis];
        }
        out
    }

    fn idle(&self) -> bool {
        self.rate.iter().all(|r| *r == 0.0) && self.pending.iter().all(|p| p.abs() < 0.01)
    }
}

/// Takes the window's wheel steps over from Slint and scrolls smoothly.
pub fn install<C: ComponentHandle + 'static>(ui: &C) {
    let pacer = Rc::new(RefCell::new(Pacer::default()));
    // Where the mouse is, in logical pixels: the scrolling goes there.
    let cursor = Rc::new(Cell::new(LogicalPosition::default()));
    let timer = Rc::new(Timer::default());
    let weak = ui.as_weak();
    ui.window().on_winit_window_event(move |window, event| match event {
        WinitEvent::CursorMoved { position, .. } => {
            let scale = window.scale_factor();
            cursor.set(LogicalPosition::new(
                position.x as f32 / scale,
                position.y as f32 / scale,
            ));
            EventResult::Propagate
        }
        WinitEvent::MouseWheel {
            delta: MouseScrollDelta::LineDelta(x, y),
            ..
        } => {
            pacer
                .borrow_mut()
                .push(Instant::now(), [x * LINE, y * LINE]);
            if !timer.running() {
                let (pacer, cursor, weak) = (pacer.clone(), cursor.clone(), weak.clone());
                let stop = Rc::downgrade(&timer);
                let last = Cell::new(Instant::now());
                timer.start(TimerMode::Repeated, TICK, move || {
                    let now = Instant::now();
                    let [dx, dy] = pacer.borrow_mut().take(now, now - last.replace(now));
                    if let Some(ui) = weak.upgrade() {
                        if dx != 0.0 || dy != 0.0 {
                            ui.window().dispatch_event(WindowEvent::PointerScrolled {
                                position: cursor.get(),
                                delta_x: dx,
                                delta_y: dy,
                            });
                        }
                    }
                    if pacer.borrow().idle() {
                        if let Some(timer) = stop.upgrade() {
                            timer.stop();
                        }
                    }
                });
            }
            EventResult::PreventDefault
        }
        // A touchpad that reports pixels scrolls the way Slint does it.
        _ => EventResult::Propagate,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a pacer through steps given as (ms after the start, pixels),
    /// ticking every 8 ms, and returns how far each tick scrolled.
    fn run(steps: &[(u64, f32)], until: u64) -> Vec<f32> {
        let start = Instant::now();
        let mut pacer = Pacer::default();
        let mut moved = Vec::new();
        let mut next = steps.iter().peekable();
        let mut t = 0;
        while t <= until {
            while let Some((at, px)) = next.next_if(|(at, _)| *at <= t) {
                pacer.push(start + Duration::from_millis(*at), [0.0, *px]);
            }
            let now = start + Duration::from_millis(t);
            moved.push(pacer.take(now, Duration::from_millis(8))[1]);
            t += 8;
        }
        moved
    }

    #[test]
    fn touchpad_coasting_keeps_an_even_speed() {
        // As recorded from a precision touchpad: a quick stream while the
        // finger is down, then, coasting, a big step every 190 ms.
        let mut steps: Vec<(u64, f32)> = (0..8).map(|i| (i * 8, -300.0)).collect();
        steps.extend((1..=5).map(|i| (56 + i * 190, -3000.0)));
        let moved = run(&steps, 56 + 5 * 190);
        // From the flick to the last coasting step, the list never stops,
        // not even when the finger leaves the pad...
        let all = &moved[..((56 + 5 * 190) / 8)];
        assert!(all.iter().all(|m| *m < 0.0), "{moved:?}");
        // ...and while it coasts, it hardly changes speed.
        let between = &moved[((56 + 190) / 8 + 1)..((56 + 5 * 190) / 8)];
        let fastest = between.iter().map(|m| m.abs()).fold(0.0, f32::max);
        let slowest = between.iter().map(|m| m.abs()).fold(f32::MAX, f32::min);
        assert!(fastest / slowest < 1.5, "{fastest} / {slowest}");
    }

    #[test]
    fn a_late_first_coasting_step_does_not_stop_the_list() {
        // As recorded: the finger leaves the pad, and the first coasting
        // step comes 315 ms later, carrying the distance for the wait.
        // Then the coasting tails off, as it does, in ever smaller steps.
        let mut steps: Vec<(u64, f32)> = (0..8).map(|i| (i * 8, -180.0)).collect();
        let mut at = 56 + 315;
        for px in [-6000.0, -1500.0, -500.0, -150.0, -60.0, -30.0] {
            steps.push((at, px));
            at += 110;
        }
        let moved = run(&steps, 56 + 315 + 110);
        assert!(moved.iter().all(|m| *m < 0.0), "{moved:?}");
        // All of it is scrolled in the end, and little more.
        let input: f32 = -steps.iter().map(|s| s.1).sum::<f32>();
        let total: f32 = -run(&steps, 3000).iter().sum::<f32>();
        assert!(total >= input - 1.0 && total < input * 1.1, "{total} of {input}");
    }

    #[test]
    fn credit_is_bounded_when_no_coasting_comes() {
        // A fast flick, then nothing (a touchpad always coasts after one,
        // but if not): the list goes on a little, then stops for good.
        let steps: Vec<(u64, f32)> = (0..8).map(|i| (i * 8, -180.0)).collect();
        let moved = run(&steps, 2000);
        let total: f32 = -moved.iter().sum::<f32>();
        assert!((1440.0..1440.0 * 1.75).contains(&total), "{total}");
        let last = moved.iter().rposition(|m| *m != 0.0).unwrap() * 8;
        assert!(last <= 56 + 350 + 8, "{last}");
    }

    #[test]
    fn turning_around_drops_the_rest() {
        let steps = [(0, -120.0), (8, -400.0), (16, -900.0), (200, -3000.0), (390, 250.0)];
        let moved = run(&steps, 1000);
        // After the turn, nothing more goes down, and exactly the new
        // step goes up.
        let after = &moved[(392 / 8)..];
        assert!(after.iter().all(|m| *m >= 0.0), "{after:?}");
        assert!((after.iter().sum::<f32>() - 250.0).abs() < 0.01);
    }

    #[test]
    fn slow_scrolling_is_exact() {
        // Mouse notches: no credit, every pixel arrives and no more.
        let moved = run(&[(0, -60.0), (300, -60.0), (310, -60.0)], 1000);
        assert!((moved.iter().sum::<f32>() + 180.0).abs() < 0.01);
    }

    #[test]
    fn a_single_notch_glides_briefly() {
        let moved = run(&[(0, -60.0)], 400);
        let ticks = moved.iter().filter(|m| **m != 0.0).count();
        // About 90 ms of 8 ms ticks.
        assert!((10..=13).contains(&ticks), "{ticks}");
    }
}
