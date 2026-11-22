use std::collections::{HashMap, HashSet};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Enter,
    Leave,
    EdgeEnter,
    EdgeLeave,
    Menu(bool),
    Drag(bool),
    Keyboard(bool),
    Fullscreen(bool),
    Show,
    Timer,
}
#[derive(Clone, Debug)]
pub struct AutoHide {
    pub visible: bool,
    pub enabled: bool,
    pub fullscreen: bool,
    pub deadline: Option<u64>,
    inside: bool,
    edge: bool,
    menu: bool,
    drag: bool,
    keyboard: bool,
    forced: bool,
    show_delay: u64,
    hide_delay: u64,
    pending: Option<bool>,
}
impl AutoHide {
    pub fn new(enabled: bool, show: u64, hide: u64, now: u64) -> Self {
        let mut state = Self {
            visible: true,
            enabled,
            fullscreen: false,
            deadline: None,
            inside: false,
            edge: false,
            menu: false,
            drag: false,
            keyboard: false,
            forced: false,
            show_delay: show,
            hide_delay: hide,
            pending: None,
        };
        state.reconcile(now);
        state
    }
    fn held(&self) -> bool {
        self.menu || self.drag || self.keyboard
    }
    fn clear(&mut self) {
        self.deadline = None;
        self.pending = None;
    }
    fn arm(&mut self, show: bool, now: u64) {
        if self.pending != Some(show) {
            self.pending = Some(show);
            self.deadline = Some(now.saturating_add(if show {
                self.show_delay
            } else {
                self.hide_delay
            }));
        }
    }
    fn reconcile(&mut self, now: u64) {
        if self.held() {
            self.visible = true;
            self.clear();
            return;
        }
        if self.fullscreen && !self.forced {
            self.visible = false;
            self.clear();
            return;
        }
        if !self.enabled && !self.fullscreen {
            self.visible = true;
            self.clear();
            return;
        }
        if self.inside || self.edge && !self.fullscreen {
            if self.visible || self.inside {
                self.visible = true;
                self.clear();
            } else {
                self.arm(true, now);
            }
        } else if self.visible {
            self.arm(false, now);
        } else {
            self.clear();
        }
    }
    pub fn event(&mut self, event: Event, now: u64) {
        match event {
            Event::Enter => self.inside = true,
            Event::Leave => {
                self.inside = false;
                self.forced = false;
            }
            Event::EdgeEnter => self.edge = !self.fullscreen,
            Event::EdgeLeave => self.edge = false,
            Event::Menu(value) => self.menu = value,
            Event::Drag(value) => self.drag = value,
            Event::Keyboard(value) => {
                self.keyboard = value;
                if !value {
                    self.forced = false;
                }
            }
            Event::Fullscreen(value) => {
                self.fullscreen = value;
                self.forced = false;
                if value {
                    self.edge = false;
                }
            }
            Event::Show => {
                self.forced = true;
                self.visible = true;
                self.clear();
            }
            Event::Timer => {
                if self.deadline.is_some_and(|deadline| now >= deadline) {
                    let show = self.pending.unwrap_or(false);
                    self.clear();
                    self.visible = show;
                    if !show {
                        self.forced = false;
                    }
                }
            }
        }
        self.reconcile(now);
    }
    pub fn configure(&mut self, enabled: bool, show: u64, hide: u64, now: u64) {
        self.enabled = enabled;
        self.show_delay = show;
        self.hide_delay = hide;
        self.clear();
        self.reconcile(now);
    }
}
pub fn zoom(distance: f64, size: f64, enabled: bool, reduced: bool) -> f64 {
    zoom_with(
        distance,
        size,
        enabled,
        reduced,
        &crate::settings::DockHover::default(),
    )
}

pub fn zoom_with(
    distance: f64,
    size: f64,
    enabled: bool,
    reduced: bool,
    hover: &crate::settings::DockHover,
) -> f64 {
    if !enabled || reduced || !distance.is_finite() || !size.is_finite() || size <= 0. {
        return 1.;
    }
    let strength = f64::from(hover.zoom_percent) / 100.;
    let radius = f64::from(hover.radius_percent.clamp(25, 200)) / 100.;
    1. + strength * (-0.5 * (distance / (size * radius)).powi(2)).exp()
}

pub fn ease(curve: crate::settings::HoverEasing, progress: f64) -> f64 {
    use crate::settings::HoverEasing;
    let progress = progress.clamp(0., 1.);
    match curve {
        HoverEasing::Linear => progress,
        HoverEasing::EaseOut => 1. - (1. - progress).powi(3),
        HoverEasing::EaseInOut => progress * progress * (3. - 2. * progress),
    }
}

pub fn divider_index(pinned: impl IntoIterator<Item = bool>) -> Option<usize> {
    pinned
        .into_iter()
        .position(|pinned| !pinned)
        .filter(|index| *index != 0)
}
pub fn fitting_icon_size(requested: u32, count: usize, width: u32) -> u32 {
    let requested = requested.clamp(32, 80);
    if count == 0 {
        return requested;
    }
    (32..=requested)
        .rev()
        .find(|size| {
            let slot = u64::from(size + 14_u32.max(size / 4 + 2));
            (count as u64).saturating_mul(slot).saturating_add(112) <= u64::from(width)
        })
        .unwrap_or(32)
}
pub fn pin_before_at<'a>(
    slots: impl IntoIterator<Item = (&'a str, f64)>,
    position: f64,
) -> &'a str {
    slots
        .into_iter()
        .find(|(_, midpoint)| position <= *midpoint)
        .map_or("", |(id, _)| id)
}

pub fn reorder(pins: &[String], source: &str, before: &str) -> Result<Vec<String>, String> {
    let mut result = crate::desktop::dock_order(pins, &[]);
    if !result.iter().any(|p| p == source)
        || !before.is_empty() && !result.iter().any(|p| p == before)
    {
        return Err("Закрепление не найдено".into());
    }
    if source == before {
        return Ok(result);
    }
    result.retain(|p| p != source);
    let at = result
        .iter()
        .position(|p| p == before)
        .unwrap_or(result.len());
    result.insert(at, source.to_owned());
    Ok(result)
}
#[derive(Default)]
pub struct Launches {
    next: u64,
    pending: HashMap<String, u64>,
}
impl Launches {
    pub fn start(&mut self, id: &str) -> u64 {
        self.next = self.next.wrapping_add(1);
        if self.next == 0 {
            self.pending.clear();
            self.next = 1;
        }
        self.pending.insert(id.to_owned(), self.next);
        self.next
    }
    pub fn expire(&mut self, id: &str, token: u64) -> bool {
        if self.pending.get(id) == Some(&token) {
            self.pending.remove(id);
            true
        } else {
            false
        }
    }
    pub fn contains(&self, id: &str) -> bool {
        self.pending.contains_key(id)
    }
    pub fn acknowledge(&mut self, opened: &HashSet<String>) {
        self.pending.retain(|id, _| !opened.contains(id));
    }
}
