use gtk::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const SLIDE_DURATION_US: i64 = 200_000;

#[derive(Clone, Copy)]
struct Slide {
    from: (f64, f64),
    to: (f64, f64),
    started: i64,
}

impl Slide {
    fn position(self, frame_time: i64) -> ((f64, f64), bool) {
        let progress =
            ((frame_time - self.started) as f64 / SLIDE_DURATION_US as f64).clamp(0., 1.);
        let eased = 1. - (1. - progress).powi(3);
        (
            (
                self.from.0 + (self.to.0 - self.from.0) * eased,
                self.from.1 + (self.to.1 - self.from.1) * eased,
            ),
            progress >= 1.,
        )
    }
}

pub struct WorkspaceIndicator {
    pub overlay: gtk::Overlay,
    pub marker: gtk::Box,
    fixed: gtk::Fixed,
    slide: Rc<RefCell<Option<Slide>>>,
    ticking: Rc<Cell<bool>>,
}

impl WorkspaceIndicator {
    pub fn new(buttons: &gtk::Box) -> Self {
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(buttons));
        let fixed = gtk::Fixed::new();
        fixed.set_can_target(false);
        let marker = gtk::Box::new(gtk::Orientation::Vertical, 0);
        marker.add_css_class("workspace-highlight");
        marker.set_can_target(false);
        marker.set_visible(false);
        fixed.put(&marker, 0., 0.);
        overlay.add_overlay(&fixed);
        overlay.set_measure_overlay(&fixed, false);
        overlay.set_clip_overlay(&fixed, true);
        Self {
            overlay,
            marker,
            fixed,
            slide: Rc::new(RefCell::new(None)),
            ticking: Rc::new(Cell::new(false)),
        }
    }

    pub fn update(&self, active: Option<&gtk::Button>) {
        let Some(active) = active else {
            self.marker.set_visible(false);
            self.slide.borrow_mut().take();
            return;
        };
        let Some(bounds) = active.compute_bounds(&self.fixed) else {
            return;
        };
        if bounds.width() <= 0. || bounds.height() <= 0. {
            return;
        }
        self.marker
            .set_size_request(bounds.width().ceil() as i32, bounds.height().ceil() as i32);
        if active.has_css_class("urgent") {
            self.marker.add_css_class("urgent");
        } else {
            self.marker.remove_css_class("urgent");
        }
        self.marker.set_visible(true);
        let target = (f64::from(bounds.x()), f64::from(bounds.y()));
        let now = self
            .fixed
            .frame_clock()
            .map(|clock| clock.frame_time())
            .unwrap_or(0);
        let animate = self.overlay.is_mapped()
            && self
                .overlay
                .settings()
                .property::<bool>("gtk-enable-animations");
        let previous = *self.slide.borrow();
        if previous.is_some_and(|slide| slide.to == target && (animate || slide.position(now).1)) {
            return;
        }
        let from = if animate {
            previous
                .map(|slide| slide.position(now).0)
                .unwrap_or(target)
        } else {
            target
        };
        *self.slide.borrow_mut() = Some(Slide {
            from,
            to: target,
            started: if from == target {
                now - SLIDE_DURATION_US
            } else {
                now
            },
        });
        self.fixed.move_(&self.marker, from.0, from.1);
        if from == target || self.ticking.replace(true) {
            return;
        }
        let marker = self.marker.downgrade();
        let slide = self.slide.clone();
        let ticking = self.ticking.clone();
        self.fixed.add_tick_callback(move |fixed, clock| {
            let next = *slide.borrow();
            let Some((marker, next)) = marker.upgrade().zip(next) else {
                ticking.set(false);
                return glib::ControlFlow::Break;
            };
            let (position, finished) = next.position(clock.frame_time());
            fixed.move_(&marker, position.0, position.1);
            if finished {
                ticking.set(false);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    pub fn is_animating(&self) -> bool {
        self.ticking.get()
            && self.marker.is_mapped()
            && self.fixed.frame_clock().is_some_and(|clock| {
                self.slide
                    .borrow()
                    .is_some_and(|slide| !slide.position(clock.frame_time()).1)
            })
    }
}
