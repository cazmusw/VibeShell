pub enum Navigation {
    Open(String),
    Back,
    Forward,
    Reload,
}

pub struct History {
    locations: Vec<String>,
    index: usize,
}

impl History {
    pub fn new(first: String) -> Self {
        Self {
            locations: vec![first],
            index: 0,
        }
    }

    pub fn current(&self) -> &str {
        &self.locations[self.index]
    }

    pub fn destination<'a>(&'a self, navigation: &'a Navigation) -> Option<&'a str> {
        match navigation {
            Navigation::Open(uri) => Some(uri),
            Navigation::Back => self
                .index
                .checked_sub(1)
                .map(|previous| self.locations[previous].as_str()),
            Navigation::Forward => self.locations.get(self.index + 1).map(String::as_str),
            Navigation::Reload => Some(self.current()),
        }
    }

    // Commit only after the directory is available.
    pub fn commit(&mut self, navigation: Navigation) {
        match navigation {
            Navigation::Open(uri) if uri != self.current() => {
                self.locations.truncate(self.index + 1);
                self.locations.push(uri);
                self.index += 1;
            }
            Navigation::Back if self.index > 0 => self.index -= 1,
            Navigation::Forward if self.index + 1 < self.locations.len() => self.index += 1,
            _ => {}
        }
    }
}
