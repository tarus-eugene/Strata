use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc, time::Instant};

pub const BAR_WIDTH: f32 = 12.;

#[derive(Clone)]
pub struct ScrollState {
    handle: ScrollHandle,
    motion: Rc<RefCell<Motion>>,
}

struct Motion {
    target: f32,
    frame: Option<Instant>,
    grab: Option<f32>,
    maximum: f32,
    animate: bool,
}

impl Default for ScrollState {
    fn default() -> Self {
        use windows::{Win32::UI::WindowsAndMessaging::*, core::BOOL};
        let mut enabled = BOOL(1);
        unsafe {
            let _ = SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some((&mut enabled as *mut BOOL).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
        }
        Self {
            handle: ScrollHandle::new(),
            motion: Rc::new(RefCell::new(Motion {
                target: 0.,
                frame: None,
                grab: None,
                maximum: 0.,
                animate: enabled.as_bool(),
            })),
        }
    }
}

fn approach(current: f32, target: f32, seconds: f32) -> f32 {
    let next = current + (target - current) * (1. - (-seconds / 0.045).exp());
    if (next - target).abs() < 0.25 {
        target
    } else {
        next
    }
}

fn wheel_target(current: f32, pending: f32, delta: f32, maximum: f32) -> f32 {
    let base = if (pending - current) * delta < 0. {
        current
    } else {
        pending
    };
    (base + delta).clamp(0., maximum.max(0.))
}

fn thumb(viewport: f32, maximum: f32, position: f32) -> (f32, f32) {
    let height = (viewport * viewport / (viewport + maximum)).clamp(28_f32.min(viewport), viewport);
    let top = position.clamp(0., maximum) / maximum * (viewport - height);
    (top, height)
}

impl ScrollState {
    fn jump(&self, position: f32) {
        let maximum: f32 = self.handle.max_offset().height.into();
        let position = position.clamp(0., maximum.max(0.));
        let mut motion = self.motion.borrow_mut();
        motion.target = position;
        motion.frame = None;
        self.handle.set_offset(point(px(0.), px(-position)));
    }

    fn wheel(&self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut App, view: EntityId) {
        let maximum: f32 = self.handle.max_offset().height.into();
        if maximum <= 0. {
            return;
        }
        let delta: f32 = (-event.delta.pixel_delta(window.line_height()).y).into();
        let current: f32 = (-self.handle.offset().y).into();
        if event.delta.precise() || !self.motion.borrow().animate {
            self.jump(current + delta);
        } else {
            let mut motion = self.motion.borrow_mut();
            let pending = if motion.frame.is_some() {
                motion.target
            } else {
                current
            };
            motion.target = wheel_target(current, pending, delta, maximum);
            motion.frame.get_or_insert_with(Instant::now);
        }
        cx.stop_propagation();
        cx.notify(view);
    }

    fn advance(&self) -> bool {
        let mut motion = self.motion.borrow_mut();
        let Some(previous) = motion.frame else {
            return false;
        };
        let now = Instant::now();
        let maximum: f32 = self.handle.max_offset().height.into();
        motion.target = motion.target.clamp(0., maximum.max(0.));
        let current: f32 = (-self.handle.offset().y).into();
        let next = approach(
            current,
            motion.target,
            now.duration_since(previous).as_secs_f32().min(0.05),
        );
        self.handle.set_offset(point(px(0.), px(-next)));
        motion.frame = if next == motion.target {
            None
        } else {
            Some(now)
        };
        motion.frame.is_some()
    }
}

pub fn scroll_area(
    id: &'static str,
    state: &ScrollState,
    content: Div,
    window: &mut Window,
    view: EntityId,
) -> Div {
    if state.advance() {
        window.on_next_frame(move |_, cx| cx.notify(view));
    }
    let wheel = state.clone();
    let bar_wheel = state.clone();
    let bar_keys = state.clone();
    let geometry = state.clone();
    let paint = state.clone();
    let maximum: f32 = state.handle.max_offset().height.into();
    div()
        .relative()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .id((id, 0usize))
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_y_scroll()
                .scrollbar_width(px(BAR_WIDTH))
                .track_scroll(&state.handle)
                .child(
                    div()
                        .w_full()
                        .on_scroll_wheel(move |event, window, cx| {
                            wheel.wheel(event, window, cx, view)
                        })
                        .child(content),
                ),
        )
        .child(
            div()
                .id((id, 1usize))
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(BAR_WIDTH))
                .when(maximum > 0., |bar| {
                    bar.focusable()
                        .focus(|style| style.border_1().border_color(rgb(0xe85002)))
                        .on_key_down(move |event, _, cx| {
                            let current: f32 = (-bar_keys.handle.offset().y).into();
                            let target = match event.keystroke.key.as_str() {
                                "up" => current - 40.,
                                "down" => current + 40.,
                                "pageup" => current - 300.,
                                "pagedown" => current + 300.,
                                "home" => 0.,
                                "end" => f32::from(bar_keys.handle.max_offset().height),
                                _ => return,
                            };
                            bar_keys.jump(target);
                            cx.stop_propagation();
                            cx.notify(view);
                        })
                })
                .on_scroll_wheel(move |event, window, cx| bar_wheel.wheel(event, window, cx, view))
                .child(
                    canvas(
                        move |bounds, window, _| {
                            (geometry.handle.max_offset().height > px(0.))
                                .then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal))
                        },
                        move |bounds, hitbox, window, cx| {
                            let maximum: f32 = paint.handle.max_offset().height.into();
                            {
                                let mut motion = paint.motion.borrow_mut();
                                if motion.maximum != maximum {
                                    motion.maximum = maximum;
                                    cx.notify(view);
                                }
                            }
                            let Some(hitbox) = hitbox else {
                                return;
                            };
                            let viewport: f32 = bounds.size.height.into();
                            if viewport <= 0. || maximum <= 0. {
                                return;
                            }
                            let position: f32 = (-paint.handle.offset().y).into();
                            let (top, height) = thumb(viewport, maximum, position);
                            let thumb_bounds = Bounds::new(
                                point(bounds.origin.x + px(3.), bounds.origin.y + px(top)),
                                size(px(6.), px(height)),
                            );
                            window.paint_quad(fill(bounds, rgb(0x191919)));
                            window.paint_quad(fill(
                                thumb_bounds,
                                rgb(
                                    if paint.motion.borrow().grab.is_some()
                                        || thumb_bounds.contains(&window.mouse_position())
                                    {
                                        0xa7a7a7
                                    } else {
                                        0x646464
                                    },
                                ),
                            ));
                            let down = paint.clone();
                            window.on_mouse_event(
                                move |event: &MouseDownEvent, phase, window, cx| {
                                    if phase != DispatchPhase::Bubble
                                        || event.button != MouseButton::Left
                                        || !hitbox.is_hovered(window)
                                    {
                                        return;
                                    }
                                    let grab: f32 = if thumb_bounds.contains(&event.position) {
                                        (event.position.y - thumb_bounds.origin.y).into()
                                    } else {
                                        height / 2.
                                    };
                                    let y: f32 = (event.position.y - bounds.origin.y).into();
                                    down.jump((y - grab) / (viewport - height).max(1.) * maximum);
                                    down.motion.borrow_mut().grab = Some(grab);
                                    cx.stop_propagation();
                                    cx.notify(view);
                                },
                            );
                            let moving = paint.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase != DispatchPhase::Bubble {
                                    return;
                                }
                                let grab = moving.motion.borrow().grab;
                                if let Some(grab) = grab {
                                    if event.dragging() {
                                        let y: f32 = (event.position.y - bounds.origin.y).into();
                                        moving.jump(
                                            (y - grab) / (viewport - height).max(1.) * maximum,
                                        );
                                    } else {
                                        moving.motion.borrow_mut().grab = None;
                                    }
                                    cx.stop_propagation();
                                    cx.notify(view);
                                }
                            });
                            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble
                                    && event.button == MouseButton::Left
                                    && paint.motion.borrow_mut().grab.take().is_some()
                                {
                                    cx.stop_propagation();
                                    cx.notify(view);
                                }
                            });
                        },
                    )
                    .size_full(),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::{approach, thumb, wheel_target};
    #[test]
    fn wheel_accumulates_clamps_and_reverses_immediately() {
        assert_eq!(wheel_target(0., 100., 60., 500.), 160.);
        assert_eq!(wheel_target(40., 100., -20., 500.), 20.);
        assert_eq!(wheel_target(0., 0., -60., 500.), 0.);
        assert_eq!(wheel_target(450., 480., 60., 500.), 500.);
    }
    #[test]
    fn easing_is_frame_rate_independent_and_settles_exactly() {
        let whole = approach(0., 100., 0.032);
        let halves = approach(approach(0., 100., 0.016), 100., 0.016);
        assert!((whole - halves).abs() < 0.001);
        assert!(whole > 0. && whole < 100.);
        assert_eq!(approach(99.9, 100., 0.016), 100.);
    }
    #[test]
    fn scrollbar_size_and_endpoints_match_scroll_range() {
        assert_eq!(thumb(200., 600., 0.), (0., 50.));
        assert_eq!(thumb(200., 600., 600.), (150., 50.));
        assert_eq!(thumb(100., 100_000., 100_000.), (72., 28.));
    }
}
