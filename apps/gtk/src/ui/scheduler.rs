use std::time::Duration;

pub(crate) fn every(
    interval: Duration,
    f: impl FnMut() -> gtk::glib::ControlFlow + 'static,
) -> gtk::glib::SourceId {
    gtk::glib::timeout_add_local(interval, f)
}

pub(crate) fn once(delay: Duration, f: impl FnOnce() + 'static) {
    gtk::glib::timeout_add_local_once(delay, f);
}

pub(crate) fn idle_once(f: impl FnOnce() + 'static) {
    gtk::glib::idle_add_local_once(f);
}
