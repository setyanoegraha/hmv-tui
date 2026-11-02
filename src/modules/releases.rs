//! Upcoming machine release schedule, scraped from `/nextreleases`.

use anyhow::Result;
use scraper::{Html, Selector};

use crate::modules::session::HmvSession;

#[derive(Debug, Clone)]
pub struct Release {
    pub date: String,
    pub name: String,
    pub level: String,
}

pub struct ReleaseScraper {
    session: HmvSession,
}

impl ReleaseScraper {
    pub fn new(session: HmvSession) -> Self {
        Self { session }
    }

    pub async fn get_releases(&self) -> Result<Vec<Release>> {
        let fetched = self.session.get("/nextreleases").await?;
        Ok(parse_releases(&fetched.text))
    }
}

/// Pure parsing function so it can be unit-tested against fixture HTML.
pub fn parse_releases(html: &str) -> Vec<Release> {
    let doc = Html::parse_document(html);
    let row_sel = Selector::parse("div.submission-row").unwrap();

    let mut releases = Vec::new();
    for row in doc.select(&row_sel) {
        // Each row repeats its values in a desktop grid and a mobile grid;
        // only the first child element (the desktop grid) is read.
        let Some(grid) = row.child_elements().next() else {
            continue;
        };
        let cells: Vec<String> = grid
            .child_elements()
            .map(|cell| cell.text().collect::<String>().trim().to_string())
            .collect();
        if cells.len() < 3 {
            continue;
        }
        releases.push(Release {
            name: cells[0].clone(),
            level: strip_dot(&cells[1]),
            date: cells[2].clone(),
        });
    }
    releases
}

/// Drops the site's leading `●` difficulty marker.
fn strip_dot(text: &str) -> String {
    text.trim().trim_start_matches('●').trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
    <html><body><section>
      <div class="submission-row border-b border-white/[0.045] px-4 py-5">
        <div class="hidden grid-cols-[minmax(0,1.3fr)_minmax(90px,0.8fr)_minmax(90px,0.8fr)] md:grid">
          <div class="min-w-0 truncate text-sm text-pink-300">Discuss</div>
          <div class="text-xs"><span class="text-yellow-400">● Medium</span></div>
          <div class="text-xs text-zinc-500">05th Nov 09:00</div>
        </div>
        <div class="grid grid-cols-2 gap-x-4 gap-y-4 md:hidden">
          <div><div class="mobile-label">VMNAME</div><div>Discuss</div></div>
          <div><div class="mobile-label">LEVEL</div><div>Medium</div></div>
          <div><div class="mobile-label">DATE</div><div>05th Nov 09:00</div></div>
        </div>
      </div>
    </section></body></html>"#;

    #[test]
    fn parses_row_triplet() {
        let releases = parse_releases(FIXTURE);
        assert_eq!(
            releases.len(),
            1,
            "mobile duplicate grid must not be counted"
        );
        assert_eq!(releases[0].name, "Discuss");
        assert_eq!(releases[0].level, "Medium");
        assert_eq!(releases[0].date, "05th Nov 09:00");
    }
}
