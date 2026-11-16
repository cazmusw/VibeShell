mod worker;
use serde::Serialize;
pub use worker::*;
#[derive(Debug, Clone, Default, Serialize)]
pub struct Capabilities {
    pub globals: Vec<(String, u32)>,
}
impl Capabilities {
    pub fn from_globals(mut globals: Vec<(String, u32)>) -> Self {
        globals.sort();
        Self { globals }
    }
    pub fn missing(&self) -> Vec<&'static str> {
        [
            "wl_compositor",
            "wl_shm",
            "xdg_wm_base",
            "zwlr_layer_shell_v1",
            "zwlr_foreign_toplevel_manager_v1",
        ]
        .into_iter()
        .filter(|name| !self.globals.iter().any(|(s, _)| s == name))
        .collect()
    }
    pub fn ready(&self) -> bool {
        self.missing().is_empty()
    }
}
#[derive(Debug, Clone, Copy)]
pub enum WindowCommand {
    Activate { epoch: u64, id: u32 },
    Minimize { epoch: u64, id: u32 },
    Close { epoch: u64, id: u32 },
}
impl WindowCommand {
    pub fn target(&self) -> (u64, u32) {
        match *self {
            Self::Activate { epoch, id }
            | Self::Minimize { epoch, id }
            | Self::Close { epoch, id } => (epoch, id),
        }
    }
}
