//! Pure frame interaction state machine and edge-resize geometry.
//! Window messages only translate input into these transitions.

use dt_common::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Move,
    Pinned,
    Input,
    Drag,
    Tune,
}

impl Mode {
    pub fn is_resize(self) -> bool {
        matches!(self, Mode::Input | Mode::Drag | Mode::Tune)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    ResizeHotkey { first_entry: bool },
    MoveHotkey,
    PinHotkey,
    Escape,
    Defocus,
    BorderPress,
    InputKey,
    TuneKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Up,
    Down,
}

/// The complete mode transition table. Move/resize are absolute commands;
/// Pin toggles between Pinned and Move from every state.
pub fn transition(mode: Mode, event: Event) -> Mode {
    match event {
        Event::MoveHotkey => Mode::Move,
        Event::PinHotkey => {
            if mode == Mode::Pinned {
                Mode::Move
            } else {
                Mode::Pinned
            }
        }
        Event::ResizeHotkey { first_entry } => {
            if mode.is_resize() {
                Mode::Input
            } else if first_entry {
                Mode::Tune
            } else {
                Mode::Drag
            }
        }
        Event::Escape => match mode {
            Mode::Input => Mode::Drag,
            Mode::Drag | Mode::Tune => Mode::Move,
            other => other,
        },
        Event::Defocus => {
            if mode.is_resize() {
                Mode::Move
            } else {
                mode
            }
        }
        Event::BorderPress => match mode {
            Mode::Move | Mode::Input | Mode::Tune => Mode::Drag,
            other => other,
        },
        Event::InputKey => match mode {
            Mode::Drag | Mode::Tune => Mode::Input,
            other => other,
        },
        Event::TuneKey => {
            if mode.is_resize() {
                Mode::Tune
            } else {
                mode
            }
        }
    }
}

/// Resize one edge outward (positive delta) or inward (negative delta),
/// keeping the opposite edge fixed and clamping to the work area / 16 px.
pub fn resize_edge(cur: Rect, work: Rect, edge: Edge, delta: i32) -> Rect {
    let mut r = cur;
    match edge {
        Edge::Left => r.l = (cur.l - delta).clamp(work.l, cur.r - 16),
        Edge::Right => r.r = (cur.r + delta).clamp(cur.l + 16, work.r),
        Edge::Up => r.t = (cur.t - delta).clamp(work.t, cur.b - 16),
        Edge::Down => r.b = (cur.b + delta).clamp(cur.t + 16, work.b),
    }
    r
}

/// Mouse-drag geometry for one or two edges (a corner). dx/dy are screen
/// pixels moved by the pointer; positive is right/down.
pub fn resize_drag(
    cur: Rect,
    work: Rect,
    horizontal: Option<Edge>,
    vertical: Option<Edge>,
    dx: i32,
    dy: i32,
) -> Rect {
    let mut r = cur;
    if let Some(edge) = horizontal {
        let delta = if edge == Edge::Left { -dx } else { dx };
        r = resize_edge(r, work, edge, delta);
    }
    if let Some(edge) = vertical {
        let delta = if edge == Edge::Up { -dy } else { dy };
        r = resize_edge(r, work, edge, delta);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESIZE: [Mode; 3] = [Mode::Input, Mode::Drag, Mode::Tune];

    #[test]
    fn mode_hotkeys_are_absolute_and_pin_toggles_with_move() {
        for from in [
            Mode::Move,
            Mode::Pinned,
            Mode::Input,
            Mode::Drag,
            Mode::Tune,
        ] {
            assert_eq!(transition(from, Event::MoveHotkey), Mode::Move);
            assert_eq!(
                transition(from, Event::PinHotkey),
                if from == Mode::Pinned {
                    Mode::Move
                } else {
                    Mode::Pinned
                }
            );
            assert_eq!(
                transition(from, Event::ResizeHotkey { first_entry: false }),
                if from.is_resize() {
                    Mode::Input
                } else {
                    Mode::Drag
                }
            );
        }
    }

    #[test]
    fn first_resize_entry_lands_on_tune() {
        assert_eq!(
            transition(Mode::Move, Event::ResizeHotkey { first_entry: true }),
            Mode::Tune
        );
    }

    #[test]
    fn escape_and_defocus_have_no_dead_ends() {
        assert_eq!(transition(Mode::Input, Event::Escape), Mode::Drag);
        assert_eq!(transition(Mode::Drag, Event::Escape), Mode::Move);
        assert_eq!(transition(Mode::Tune, Event::Escape), Mode::Move);
        for mode in RESIZE {
            assert_eq!(transition(mode, Event::Defocus), Mode::Move);
        }
    }

    #[test]
    fn border_press_normalizes_resize_to_drag() {
        assert_eq!(transition(Mode::Move, Event::BorderPress), Mode::Drag);
        for mode in RESIZE {
            assert_eq!(transition(mode, Event::BorderPress), Mode::Drag);
        }
    }

    #[test]
    fn input_and_tune_keys_only_apply_inside_resize() {
        assert_eq!(transition(Mode::Drag, Event::InputKey), Mode::Input);
        assert_eq!(transition(Mode::Tune, Event::InputKey), Mode::Input);
        assert_eq!(transition(Mode::Input, Event::TuneKey), Mode::Tune);
        assert_eq!(transition(Mode::Move, Event::InputKey), Mode::Move);
        assert_eq!(transition(Mode::Pinned, Event::TuneKey), Mode::Pinned);
    }

    #[test]
    fn edge_resize_moves_only_the_selected_edge() {
        let cur = Rect {
            l: 100,
            t: 100,
            r: 300,
            b: 250,
        };
        let work = Rect {
            l: 0,
            t: 0,
            r: 1000,
            b: 700,
        };
        let r = resize_edge(cur, work, Edge::Right, 5);
        assert_eq!((r.l, r.r, r.t, r.b), (100, 305, 100, 250));
        let r = resize_edge(cur, work, Edge::Left, 5);
        assert_eq!((r.l, r.r), (95, 300));
        let r = resize_edge(cur, work, Edge::Up, 5);
        assert_eq!((r.t, r.b), (95, 250));
        let r = resize_edge(cur, work, Edge::Down, -5);
        assert_eq!((r.t, r.b), (100, 245));
    }

    #[test]
    fn corner_drag_moves_both_edges_and_keeps_the_opposite_corner() {
        let cur = Rect {
            l: 100,
            t: 100,
            r: 300,
            b: 250,
        };
        let work = Rect {
            l: 0,
            t: 0,
            r: 1000,
            b: 700,
        };
        let r = resize_drag(cur, work, Some(Edge::Right), Some(Edge::Down), 20, 30);
        assert_eq!((r.l, r.t, r.r, r.b), (100, 100, 320, 280));
        let r = resize_drag(cur, work, Some(Edge::Left), Some(Edge::Up), 20, 30);
        assert_eq!((r.l, r.t, r.r, r.b), (120, 130, 300, 250));
    }

    #[test]
    fn edge_resize_clamps_to_work_area_and_minimum() {
        let work = Rect {
            l: 0,
            t: 0,
            r: 1000,
            b: 700,
        };
        let cur = Rect {
            l: 100,
            t: 100,
            r: 120,
            b: 130,
        };
        let r = resize_edge(cur, work, Edge::Right, 5000);
        assert_eq!(r.r, 1000);
        let r = resize_edge(cur, work, Edge::Right, -5000);
        assert_eq!((r.l, r.r), (100, 116));
        let r = resize_edge(cur, work, Edge::Left, -5000);
        assert_eq!((r.l, r.r), (104, 120));
    }
}
