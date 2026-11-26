//! Your notch settings: which pages show, and in what order. Saved to a small text file
//! (%APPDATA%\DynamicIsland\settings.ini) so they survive restarts.

use crate::Page;

/// Every page you can arrange (Home is always first and always on).
const ALL: [Page; 5] = [Page::Music, Page::Shelf, Page::Controls, Page::Network, Page::System];

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Page order after Home.
    pub order: Vec<Page>,
    pub hidden: Vec<Page>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { order: ALL.to_vec(), hidden: Vec::new() }
    }
}

pub fn name(page: Page) -> &'static str {
    match page {
        Page::Home => "Home",
        Page::Music => "Music",
        Page::Shelf => "File shelf",
        Page::Controls => "Controls",
        Page::Network => "Network",
        Page::System => "System",
    }
}

fn key(page: Page) -> &'static str {
    match page {
        Page::Home => "home",
        Page::Music => "music",
        Page::Shelf => "shelf",
        Page::Controls => "controls",
        Page::Network => "network",
        Page::System => "system",
    }
}

fn from_key(text: &str) -> Option<Page> {
    ALL.iter().copied().find(|p| key(*p) == text.trim())
}

fn path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(std::path::Path::new(&base).join("DynamicIsland").join("settings.ini"))
}

impl Prefs {
    pub fn load() -> Prefs {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map(|text| Prefs::parse(&text)).unwrap_or_default()
    }

    fn parse(text: &str) -> Prefs {
        let mut prefs = Prefs { order: Vec::new(), hidden: Vec::new() };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let pages: Vec<Page> = v.split(',').filter_map(from_key).collect();
            match k.trim() {
                "order" => prefs.order = pages,
                "hidden" => prefs.hidden = pages,
                _ => {}
            }
        }
        // Keep the order valid: no repeats, and any page missing from it goes at the end.
        let mut order: Vec<Page> = Vec::new();
        for p in prefs.order.into_iter().chain(ALL) {
            if !order.contains(&p) {
                order.push(p);
            }
        }
        prefs.order = order;
        prefs.hidden.dedup();
        prefs
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let list = |pages: &[Page]| pages.iter().map(|p| key(*p)).collect::<Vec<_>>().join(",");
        let _ = std::fs::write(path, format!("order={}\nhidden={}\n", list(&self.order), list(&self.hidden)));
    }

    pub fn toggle(&mut self, page: Page) {
        if let Some(i) = self.hidden.iter().position(|p| *p == page) {
            self.hidden.remove(i);
        } else {
            self.hidden.push(page);
        }
    }

    /// Move the page at `from` to position `to`.
    pub fn reorder(&mut self, from: usize, to: usize) {
        if from < self.order.len() && to < self.order.len() && from != to {
            let page = self.order.remove(from);
            self.order.insert(to, page);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fills_in_missing_pages() {
        let p = Prefs::parse("order=system,controls\nhidden=music\n");
        assert_eq!(p.order, vec![Page::System, Page::Controls, Page::Music, Page::Shelf, Page::Network]);
        assert_eq!(p.hidden, vec![Page::Music]);
    }
}
