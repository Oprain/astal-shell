use astal_shell::config::Config;
use gtk::{gdk, prelude::*};
use std::{
    cell::{Cell, RefCell},
    f64::consts::PI,
    rc::Rc,
};

pub struct Meter {
    pub row: gtk::Box,
    area: gtk::DrawingArea,
    pub value: Rc<Cell<f64>>,
    colors: Rc<RefCell<[gdk::RGBA; 3]>>,
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, radius: f64) {
    let radius = radius.min(w / 2.).min(h / 2.);
    cr.new_sub_path();
    cr.arc(x + w - radius, y + radius, radius, -PI / 2., 0.);
    cr.arc(x + w - radius, y + h - radius, radius, 0., PI / 2.);
    cr.arc(x + radius, y + h - radius, radius, PI / 2., PI);
    cr.arc(x + radius, y + radius, radius, PI, 3. * PI / 2.);
    cr.close_path();
}

impl Meter {
    pub fn new(label: &str, invert: bool, config: &Config) -> Self {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        row.set_widget_name(&format!("meter-{label}"));
        row.set_valign(gtk::Align::Center);
        let value = Rc::new(Cell::new(0_f64));
        let colors = Rc::new(RefCell::new(config.status_colors()));
        let area = gtk::DrawingArea::new();
        area.set_content_width(8);
        area.set_content_height(24);
        let level = value.clone();
        let palette = colors.clone();
        area.set_draw_func(move |_, cr, w, h| {
            rounded_rect(cr, 0., 0., f64::from(w), f64::from(h), 2.);
            cr.set_source_rgba(1., 1., 1., 0.1);
            let _ = cr.fill();
            let value = level.get().clamp(0., 1.);
            let height = (f64::from(h) * value).round();
            if height > 0. {
                let index = usize::from(value > 0.25) + usize::from(value > 0.75);
                let color = palette.borrow()[if invert { 2 - index } else { index }];
                cr.set_source_rgba(
                    f64::from(color.red()),
                    f64::from(color.green()),
                    f64::from(color.blue()),
                    f64::from(color.alpha()),
                );
                rounded_rect(cr, 0., f64::from(h) - height, f64::from(w), height, 2.);
                let _ = cr.fill();
            }
        });
        row.append(&area);
        let caption = gtk::DrawingArea::new();
        caption.add_css_class("meter-label");
        caption.set_content_width(8);
        caption.set_content_height(24);
        let label = label.to_owned();
        caption.set_draw_func(move |widget, cr, w, h| {
            let layout = widget.create_pango_layout(Some(&label));
            let (text_width, text_height) = layout.pixel_size();
            let color = widget.color();
            cr.set_source_rgba(
                f64::from(color.red()),
                f64::from(color.green()),
                f64::from(color.blue()),
                f64::from(color.alpha()),
            );
            let _ = cr.save();
            cr.translate(f64::from(w) / 2., f64::from(h) / 2.);
            cr.rotate(PI / 2.);
            cr.move_to(-f64::from(text_width) / 2., -f64::from(text_height) / 2.);
            pangocairo::functions::show_layout(cr, &layout);
            let _ = cr.restore();
        });
        row.append(&caption);
        Self {
            row,
            area,
            value,
            colors,
        }
    }

    pub fn configure(&self, config: &Config) {
        *self.colors.borrow_mut() = config.status_colors();
        self.area.queue_draw();
    }

    pub fn set(&self, value: f64, tooltip: &str) {
        self.value.set(if value.is_finite() {
            value.clamp(0., 1.)
        } else {
            0.
        });
        self.row.set_tooltip_text(Some(tooltip));
        self.area.queue_draw();
    }
}
