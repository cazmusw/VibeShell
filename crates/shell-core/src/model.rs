use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub epoch: u64,
    pub id: u32,
    pub app_id: String,
    pub title: String,
    pub active: bool,
    pub minimized: bool,
    pub maximized: bool,
}
#[derive(Debug, Clone)]
pub enum WindowEvent {
    Reset(u64),
    Upsert(Window),
    Closed { epoch: u64, id: u32 },
}
#[derive(Default)]
pub struct WindowStore {
    epoch: u64,
    windows: BTreeMap<u32, Window>,
}
impl WindowStore {
    pub fn apply(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::Reset(epoch) => {
                self.epoch = epoch;
                self.windows.clear();
            }
            WindowEvent::Upsert(w) if w.epoch == self.epoch => {
                self.windows.insert(w.id, w);
            }
            WindowEvent::Closed { epoch, id } if epoch == self.epoch => {
                self.windows.remove(&id);
            }
            _ => {}
        }
    }
    pub fn contains(&self, epoch: u64, id: u32) -> bool {
        self.epoch == epoch && self.windows.contains_key(&id)
    }
    pub fn snapshot(&self) -> Vec<Window> {
        self.windows.values().cloned().collect()
    }
}
