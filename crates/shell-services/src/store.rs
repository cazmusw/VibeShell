use serde::Serialize;
use std::collections::VecDeque;
#[derive(Debug, Clone)]
pub struct NewNotification {
    pub sender: String,
    pub app: String,
    pub summary: String,
    pub body: String,
    pub replaces_id: u32,
    pub transient: bool,
    pub urgency: u8,
    pub timeout_ms: i32,
}
impl Default for NewNotification {
    fn default() -> Self {
        Self {
            sender: String::new(),
            app: String::new(),
            summary: String::new(),
            body: String::new(),
            replaces_id: 0,
            transient: false,
            urgency: 1,
            timeout_ms: -1,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    pub created_ms: u64,
    pub live: bool,
    pub transient: bool,
    pub urgency: u8,
    #[serde(skip)]
    pub sender: String,
    #[serde(skip)]
    pub deadline: Option<u64>,
}
#[derive(Default)]
pub struct NotificationStore {
    next: u32,
    evicted: Vec<u32>,
    items: VecDeque<Notification>,
    pub dnd: bool,
}
impl NotificationStore {
    pub fn items(&self) -> Vec<Notification> {
        self.items.iter().cloned().collect()
    }
    pub fn notify(&mut self, n: NewNotification, now: u64) -> u32 {
        let replacement = self
            .items
            .iter()
            .position(|old| old.id == n.replaces_id && old.live && old.sender == n.sender);
        let id = if let Some(pos) = replacement {
            self.items[pos].id
        } else {
            loop {
                self.next = self.next.wrapping_add(1);
                if self.next != 0 && !self.items.iter().any(|n| n.id == self.next) {
                    break self.next;
                }
            }
        };
        let timeout = match n.timeout_ms {
            0 => None,
            v if v > 0 => Some(v as u64),
            _ if n.urgency == 2 => None,
            _ => Some(5000),
        };
        let notification = Notification {
            id,
            app: truncate(n.app, 256),
            summary: truncate(n.summary, 1024),
            body: truncate(n.body, 8192),
            created_ms: now,
            live: true,
            transient: n.transient,
            urgency: n.urgency.min(2),
            sender: n.sender,
            deadline: timeout.map(|d| now.saturating_add(d)),
        };
        if let Some(pos) = replacement {
            self.items[pos] = notification
        } else {
            self.items.push_back(notification)
        }
        while self.items.len() > 500 {
            let pos = self.items.iter().position(|n| !n.live).unwrap_or(0);
            if let Some(old) = self.items.remove(pos) {
                if old.live {
                    self.evicted.push(old.id);
                }
            }
        }
        id
    }
    pub fn take_evicted(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.evicted)
    }
    pub fn close(&mut self, id: u32, sender: &str) -> Option<u32> {
        let pos = self
            .items
            .iter()
            .position(|n| n.id == id && n.live && (sender.is_empty() || n.sender == sender))?;
        if self.items[pos].transient {
            self.items.remove(pos);
        } else {
            self.items[pos].live = false;
            self.items[pos].deadline = None;
        }
        Some(id)
    }
    pub fn expire(&mut self, now: u64) -> Vec<u32> {
        let ids: Vec<_> = self
            .items
            .iter()
            .filter(|n| n.live && n.deadline.is_some_and(|d| d <= now))
            .map(|n| n.id)
            .collect();
        for id in &ids {
            self.close(*id, "");
        }
        self.items
            .retain(|n| n.live || now.saturating_sub(n.created_ms) < 7 * 24 * 60 * 60 * 1000);
        ids
    }
    pub fn clear(&mut self) {
        self.items.retain(|n| n.live)
    }
}
fn truncate(s: String, max: usize) -> String {
    s.chars().take(max).collect()
}
