#[cfg(feature = "desktop")]
pub mod audio;
#[cfg(feature = "desktop")]
mod audio_ui;
#[cfg(feature = "desktop")]
pub mod bluetooth;
#[cfg(feature = "desktop")]
mod bluetooth_ui;
#[cfg(feature = "desktop")]
pub mod brightness;
#[cfg(feature = "desktop")]
mod brightness_ui;
pub mod desktop;
pub mod diagnostics;
#[cfg(feature = "desktop")]
pub mod dock;
pub mod dock_icons;
pub mod dock_model;
pub mod dock_preview;
pub mod doctor;
#[cfg(feature = "desktop")]
pub mod files_locale;
#[cfg(feature = "desktop")]
pub mod ipc;
#[cfg(feature = "desktop")]
pub mod network;
#[cfg(feature = "desktop")]
mod network_ui;
pub mod power;
#[cfg(feature = "desktop")]
mod power_ui;
pub mod process;
pub mod settings;
pub mod temperatures;
#[cfg(feature = "desktop")]
pub mod toplevel;
#[cfg(feature = "desktop")]
pub mod ui;
pub mod wayland;
#[cfg(feature = "desktop")]
pub mod zoom_icon;

pub mod outputs;

#[cfg(feature = "desktop")]
pub mod panel;

pub mod tray_model;

#[cfg(feature = "desktop")]
pub mod trash;

#[cfg(feature = "desktop")]
pub mod tray_backend;

#[cfg(feature = "desktop")]
pub mod tray_ui;

#[cfg(feature = "desktop")]
pub mod menu_model;

#[cfg(feature = "desktop")]
pub mod tray_menu;
