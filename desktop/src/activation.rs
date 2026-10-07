//! Raising a window from the tray on Wayland. The compositor only activates a
//! window that presents an XDG activation token; without one KWin merely
//! flags it in the taskbar. Plasma hands the tray a token (through the
//! StatusNotifierItem's ProvideXdgActivationToken) just before the click
//! arrives, and GDK presents it for the next window it maps or focuses.
use gtk::{
    gdk, glib,
    glib::{
        prelude::*,
        translate::{FromGlib as _, ToGlibPtr as _},
    },
};
use std::{cell::Cell, ffi::CString};

thread_local! {
    /// A token is waiting in GDK for the next window it maps or focuses.
    static PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Give GDK the token of the tray click being handled, if the host sent one.
/// Runs on the main (GTK) thread, where tray events are delivered.
pub fn adopt_tray_token() {
    let Some(token) = ksni::take_xdg_activation_token() else {
        return;
    };
    if !gtk::is_initialized_main_thread() {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    // SAFETY: plain type lookup; GDK is initialized on this thread.
    let wayland = unsafe { glib::Type::from_glib(gdk_wayland_sys::gdk_wayland_display_get_type()) };
    if !display.type_().is_a(wayland) {
        return;
    }
    let Ok(token) = CString::new(token) else {
        return;
    };
    let raw: *mut gdk::ffi::GdkDisplay = display.to_glib_none().0;
    // SAFETY: DISPLAY is a GdkWaylandDisplay, used on the GTK main thread;
    // GDK copies the token.
    unsafe { gdk_wayland_sys::gdk_wayland_display_set_startup_notification_id(raw.cast(), token.as_ptr()) };
    PENDING.set(true);
}

/// Whether a tray token is waiting (consumed by this call).
pub fn take_pending() -> bool {
    PENDING.replace(false)
}
