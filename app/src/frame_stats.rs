//! `JEWELCASE_FRAME_STATS=1`: logs how smoothly each animation ran, from
//! the window's frame clock. Frames painted less than 100 ms apart count
//! as one animation (one that goes on, like the visualizer, is logged
//! every two seconds); when it ends, the log says how many frames it had,
//! its frame rate, its longest frame, and how long painting took (layout,
//! snapshot and handing the frame to the renderer). Logged as warnings, so release
//! builds show them too (`journalctl --user -t jewelcase`, or the
//! terminal).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

/// A pause longer than this ends an animation.
const GAP_US: i64 = 100_000;
/// A continuous run is logged in parts this long.
const PART_US: i64 = 2_000_000;
/// Shorter runs (a single redraw, a cursor blink) aren't logged.
const MIN_FRAMES: u32 = 8;

#[derive(Default)]
struct Run {
    first_us: i64,
    last_us: i64,
    frames: u32,
    longest_us: i64,
    /// Time spent painting: in total, and the longest.
    paint_us: u64,
    longest_paint_us: u64,
    flush: Option<glib::SourceId>,
}

pub fn watch(window: &gtk::Window) {
    if std::env::var_os("JEWELCASE_FRAME_STATS").is_none() {
        return;
    }
    window.connect_realize(|window| {
        let Some(clock) = window.frame_clock() else {
            return;
        };
        let run: Rc<RefCell<Run>> = Rc::default();
        let started: Rc<RefCell<Option<Instant>>> = Rc::default();
        clock.connect_layout({
            let started = started.clone();
            move |_| {
                started.borrow_mut().get_or_insert_with(Instant::now);
            }
        });
        clock.connect_after_paint(move |clock| {
            let paint_us = started
                .take()
                .map_or(0, |at| at.elapsed().as_micros() as u64);
            painted(&run, clock.frame_time(), paint_us);
        });
    });
}

fn painted(run: &Rc<RefCell<Run>>, now_us: i64, paint_us: u64) {
    let mut state = run.borrow_mut();
    if state.frames > 0 && now_us - state.last_us < GAP_US && now_us - state.first_us < PART_US {
        state.longest_us = state.longest_us.max(now_us - state.last_us);
        state.frames += 1;
    } else {
        log(&state);
        *state = Run {
            first_us: now_us,
            last_us: now_us,
            frames: 1,
            longest_us: 0,
            paint_us: 0,
            longest_paint_us: 0,
            flush: state.flush.take(),
        };
    }
    state.paint_us += paint_us;
    state.longest_paint_us = state.longest_paint_us.max(paint_us);
    state.last_us = now_us;
    // Logged once no frame follows.
    if let Some(flush) = state.flush.take() {
        flush.remove();
    }
    let run2 = run.clone();
    state.flush = Some(glib::timeout_add_local_once(
        Duration::from_micros(GAP_US as u64),
        move || {
            let mut state = run2.borrow_mut();
            state.flush = None;
            log(&state);
            state.frames = 0;
        },
    ));
}

fn log(run: &Run) {
    if run.frames < MIN_FRAMES {
        return;
    }
    let span_ms = (run.last_us - run.first_us) as f64 / 1000.0;
    let fps = f64::from(run.frames - 1) / (span_ms / 1000.0);
    tracing::warn!(
        "Animation: {} frames in {span_ms:.0} ms, {fps:.1} fps, longest frame {:.1} ms; \
         painting {:.1} ms a frame, at most {:.1} ms",
        run.frames,
        run.longest_us as f64 / 1000.0,
        run.paint_us as f64 / 1000.0 / f64::from(run.frames),
        run.longest_paint_us as f64 / 1000.0,
    );
}
