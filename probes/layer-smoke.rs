//! Disposable stage-0 ABI probe, not production GTK bindings.
//! All pointers stay on the main thread; GTK owns widgets until destroy.
use std::{
    ffi::{CStr, c_int, c_void},
    ptr,
    time::{Duration, Instant},
};
mod toplevel;

#[link(name = "gtk-4")]
unsafe extern "C" {
    fn gtk_init_check() -> c_int;
    fn gtk_widget_realize(widget: *mut c_void);
    fn gtk_native_get_surface(native: *mut c_void) -> *mut c_void;
    fn gtk_native_get_renderer(native: *mut c_void) -> *mut c_void;
    fn gdk_surface_get_scale(surface: *mut c_void) -> f64;
    fn gdk_wayland_toplevel_set_application_id(surface: *mut c_void, id: *const std::ffi::c_char);
    fn gtk_get_major_version() -> u32;
    fn gtk_get_minor_version() -> u32;
    fn gtk_get_micro_version() -> u32;
    fn gtk_window_new() -> *mut c_void;
    fn gtk_window_is_active(window: *mut c_void) -> c_int;
    fn gtk_window_set_default_size(window: *mut c_void, width: c_int, height: c_int);
    fn gtk_window_set_title(window: *mut c_void, title: *const std::ffi::c_char);
    fn gtk_window_set_child(window: *mut c_void, child: *mut c_void);
    fn gtk_label_new(text: *const std::ffi::c_char) -> *mut c_void;
    fn gtk_window_present(window: *mut c_void);
    fn gtk_widget_get_mapped(widget: *mut c_void) -> c_int;
    fn gtk_widget_get_width(widget: *mut c_void) -> c_int;
    fn gtk_widget_get_height(widget: *mut c_void) -> c_int;
    fn gtk_window_destroy(window: *mut c_void);
    fn gtk_widget_get_display(widget: *mut c_void) -> *mut c_void;
    fn gtk_css_provider_new() -> *mut c_void;
    fn gtk_css_provider_load_from_string(provider: *mut c_void, css: *const std::ffi::c_char);
    fn gtk_style_context_add_provider_for_display(
        display: *mut c_void,
        provider: *mut c_void,
        priority: u32,
    );
}
#[link(name = "gobject-2.0")]
unsafe extern "C" {
    fn g_type_name_from_instance(instance: *mut c_void) -> *const std::ffi::c_char;
    fn g_object_ref(object: *mut c_void) -> *mut c_void;
    fn g_object_unref(object: *mut c_void);
}
#[link(name = "gtk4-layer-shell")]
unsafe extern "C" {
    fn gtk_layer_is_supported() -> c_int;
    fn gtk_layer_init_for_window(window: *mut c_void);
    fn gtk_layer_set_namespace(window: *mut c_void, name: *const std::ffi::c_char);
    fn gtk_layer_set_layer(window: *mut c_void, layer: c_int);
    fn gtk_layer_set_anchor(window: *mut c_void, edge: c_int, anchor: c_int);
    fn gtk_layer_set_keyboard_mode(window: *mut c_void, mode: c_int);
    fn gtk_layer_set_exclusive_zone(window: *mut c_void, zone: c_int);
}
#[link(name = "glib-2.0")]
unsafe extern "C" {
    fn g_main_context_iteration(context: *mut c_void, may_block: c_int) -> c_int;
    fn g_set_prgname(name: *const std::ffi::c_char);
}

fn main() {
    let mode = std::env::args().nth(1);
    let control = mode.as_deref() == Some("--window-control");
    let preview = match mode.as_deref() {
        Some("--preview-red") => Some((c"Red preview fixture", c"window { background: #fb194f; }")),
        Some("--preview-blue") => {
            Some((c"Blue preview fixture", c"window { background: #14aaf6; }"))
        }
        _ => None,
    };
    let hold = mode.as_deref() == Some("--hold-window") || preview.is_some();
    // SAFETY: ABI declarations checked against GTK 4.22.5 and layer-shell 1.3.0
    // headers. Valid GTK objects only, C string literals, single main thread.
    let passed = unsafe {
        g_set_prgname(c"mosaic-stage-0-probe".as_ptr());
        if gtk_init_check() == 0 || gtk_layer_is_supported() == 0 {
            eprintln!("GTK4 Wayland/layer-shell unavailable");
            std::process::exit(1);
        }
        println!(
            "GTK {}.{}.{}",
            gtk_get_major_version(),
            gtk_get_minor_version(),
            gtk_get_micro_version()
        );
        let window = gtk_window_new();
        assert!(!window.is_null());
        g_object_ref(window);
        let mut probe = if control || hold {
            gtk_widget_realize(window);
            gdk_wayland_toplevel_set_application_id(
                gtk_native_get_surface(window),
                c"mosaic-stage-0-probe".as_ptr(),
            );
            if control {
                Some(toplevel::Probe::new().expect("toplevel probe connection"))
            } else {
                None
            }
        } else {
            gtk_layer_init_for_window(window);
            gtk_layer_set_namespace(window, c"mosaic-stage-0-probe".as_ptr());
            gtk_layer_set_layer(window, 2); // TOP, not overlay
            gtk_layer_set_anchor(window, 2, 1); // TOP
            gtk_layer_set_keyboard_mode(window, 0); // NONE: no keyboard grab
            gtk_layer_set_exclusive_zone(window, 0); // no workspace reservation
            None
        };
        let provider = if let Some((title, css)) = preview {
            gtk_window_set_title(window, title.as_ptr());
            let provider = gtk_css_provider_new();
            gtk_css_provider_load_from_string(provider, css.as_ptr());
            gtk_style_context_add_provider_for_display(
                gtk_widget_get_display(window),
                provider,
                800,
            );
            provider
        } else {
            ptr::null_mut()
        };
        gtk_window_set_default_size(window, 480, if preview.is_some() { 270 } else { 40 });
        gtk_window_set_child(
            window,
            gtk_label_new(c"Mosaic: GTK4 layer-shell probe (2 seconds)".as_ptr()),
        );
        gtk_window_present(window);
        let deadline = Instant::now()
            + Duration::from_secs(if hold {
                20
            } else if control {
                5
            } else {
                2
            });
        let update = std::env::args_os().nth(2);
        let mut updated = false;
        let mut mapped = false;
        let mut controlled = false;
        while Instant::now() < deadline {
            for _ in 0..64 {
                if g_main_context_iteration(ptr::null_mut(), 0) == 0 {
                    break;
                }
            }
            let currently_mapped = gtk_widget_get_mapped(window) != 0;
            if hold && mapped && !currently_mapped {
                break;
            }
            mapped |= currently_mapped
                && gtk_widget_get_width(window) > 0
                && gtk_widget_get_height(window) > 0;
            if let Some(probe) = &mut probe {
                match probe.tick() {
                    Ok(true) => {
                        controlled = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        eprintln!("window-control probe: {e}");
                        break;
                    }
                }
            }
            if !provider.is_null()
                && !updated
                && update
                    .as_ref()
                    .is_some_and(|p| std::path::Path::new(p).exists())
            {
                gtk_css_provider_load_from_string(
                    provider,
                    c"window { background: #22d952; }".as_ptr(),
                );
                updated = true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        println!(
            "surface mapped={mapped}, allocation={}x{}",
            gtk_widget_get_width(window),
            gtk_widget_get_height(window)
        );
        println!("keyboard_active={}", gtk_window_is_active(window) != 0);
        let renderer = gtk_native_get_renderer(window);
        if !renderer.is_null() {
            let name = CStr::from_ptr(g_type_name_from_instance(renderer));
            println!(
                "renderer={}, surface_scale={}",
                name.to_string_lossy(),
                gdk_surface_get_scale(gtk_native_get_surface(window))
            );
        }
        gtk_window_destroy(window);
        for _ in 0..64 {
            if g_main_context_iteration(ptr::null_mut(), 0) == 0 {
                break;
            }
        }
        g_object_unref(window);
        if !provider.is_null() {
            g_object_unref(provider);
        }
        mapped && (!control || controlled)
    };
    if !passed {
        std::process::exit(1);
    }
}
