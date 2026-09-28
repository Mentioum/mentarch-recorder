//! Harmless animated window for manual portal, occlusion and resize checks.
use gtk::{glib, prelude::*};
use std::time::{Duration, Instant};

fn main() {
    let app = gtk::Application::builder()
        .application_id("io.github.mentioum.RecorderTestTarget")
        .build();
    app.connect_activate(|app| {
        let started = Instant::now();
        let area = gtk::DrawingArea::builder()
            .content_width(640)
            .content_height(360)
            .build();
        area.set_draw_func(move |_, cr, width, height| {
            cr.set_source_rgb(0.08, 0.12, 0.2);
            let _ = cr.paint();
            cr.set_source_rgb(0.3, 0.85, 0.65);
            let x =
                30.0 + (started.elapsed().as_secs_f64() * 90.0) % (width as f64 - 60.0).max(1.0);
            cr.arc(x, height as f64 * 0.65, 24.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            cr.set_source_rgb(0.95, 0.96, 1.0);
            cr.set_font_size(26.0);
            cr.move_to(24.0, 50.0);
            let _ = cr.show_text("Recorder test window");
            cr.set_font_size(18.0);
            cr.move_to(24.0, 84.0);
            let _ = cr.show_text(&format!(
                "{} × {}  |  {:.1}s",
                width,
                height,
                started.elapsed().as_secs_f64()
            ));
        });
        let weak_area = area.downgrade();
        glib::timeout_add_local(Duration::from_millis(33), move || {
            if let Some(area) = weak_area.upgrade() {
                area.queue_draw();
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Recorder test window")
            .child(&area)
            .default_width(640)
            .default_height(360)
            .build();
        window.present();
    });
    app.run();
}
