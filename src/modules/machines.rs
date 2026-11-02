//! Machine catalog: the whole `/machines` document in one request.
//!
//! Every row carries its own data attributes, so no paging and no second
//! "hacked" pass is needed.

use anyhow::Result;
use scraper::{Html, Selector};

use crate::modules::session::HmvSession;

#[derive(Debug, Clone)]
pub struct Machine {
    pub name: String,
    pub creator: String,
    /// Bare gigabytes as the site prints them, e.g. "2.5".
    pub size: String,
    /// Easy / Medium / Hard (verbatim; one row is lowercase on the site).
    pub difficulty: String,
    pub os: String,
    /// "PWNED" or "TO HACK".
    pub status: String,
}

/// GB size as a float for sorting; unknown shapes sort as 0.
pub fn size_gb(size: &str) -> f64 {
    size.trim().parse().unwrap_or(0.0)
}

pub struct MachineScraper {
    session: HmvSession,
}

impl MachineScraper {
    pub fn new(session: HmvSession) -> Self {
        Self { session }
    }

    /// Fetches the complete catalog (all rows, newest first).
    pub async fn get_catalog(&self) -> Result<Vec<Machine>> {
        let fetched = self.session.get("/machines").await?;
        Ok(parse_machines(&fetched.text))
    }
}

/// Pure parsing function so it can be unit-tested against fixture HTML.
pub fn parse_machines(html: &str) -> Vec<Machine> {
    let doc = Html::parse_document(html);
    let row_sel = Selector::parse("tr[data-machine]").unwrap();
    let td_sel = Selector::parse("td").unwrap();

    let mut machines = Vec::new();
    for row in doc.select(&row_sel) {
        let Some(name) = row.value().attr("data-name") else {
            continue;
        };
        // The size cell is the sixth column: ID, Machine, OS, Creator,
        // Difficulty, Size, Status, Open.
        let size = row
            .select(&td_sel)
            .nth(5)
            .map(|cell| cell.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        machines.push(Machine {
            name: name.to_string(),
            creator: row
                .value()
                .attr("data-creator")
                .unwrap_or("")
                .trim_start_matches('@')
                .to_string(),
            size,
            difficulty: row.value().attr("data-level").unwrap_or("").to_string(),
            os: row.value().attr("data-os").unwrap_or("").to_string(),
            status: if row.value().attr("data-finished") == Some("true") {
                "PWNED"
            } else {
                "TO HACK"
            }
            .to_string(),
        });
    }
    machines
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"
    <html><body><table>
    <tr data-machine data-name="Ledger" data-os="Linux" data-creator="m4k3r"
        data-level="Easy" data-finished="true">
      <td>#397</td><td>Ledger</td><td>Linux</td><td>@m4k3r</td><td>Easy</td>
      <td>1.2</td><td>✓ Pwned</td><td></td>
    </tr>
    <tr data-machine data-name="Madlabs" data-os="Windows" data-creator="eMVee"
        data-level="Medium" data-finished="false">
      <td>#398</td><td>Madlabs</td><td>Windows</td><td>@eMVee</td><td>Medium</td>
      <td>2.5</td><td>○ Not pwned</td><td></td>
    </tr>
    </table></body></html>"##;

    #[test]
    fn parses_catalog_rows() {
        let machines = parse_machines(FIXTURE);
        assert_eq!(machines.len(), 2);

        let ledger = &machines[0];
        assert_eq!(ledger.name, "Ledger");
        assert_eq!(ledger.creator, "m4k3r");
        assert_eq!(ledger.size, "1.2");
        assert_eq!(ledger.difficulty, "Easy");
        assert_eq!(ledger.os, "Linux");
        assert_eq!(ledger.status, "PWNED");

        let madlabs = &machines[1];
        assert_eq!(madlabs.status, "TO HACK");
        assert_eq!(madlabs.os, "Windows");
        assert_eq!(madlabs.size, "2.5");
    }

    #[test]
    fn parses_sizes_into_gigabytes() {
        assert_eq!(size_gb("2.5"), 2.5);
        assert_eq!(size_gb(" 1.0 "), 1.0);
        assert_eq!(size_gb(""), 0.0);
        assert_eq!(size_gb("garbage"), 0.0);
    }
}
