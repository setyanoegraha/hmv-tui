//! Community writeups for a specific VM and writeup submission verdicts.
//! The new platform shows only author + link per writeup; submission needs a
//! type and a language in addition to the URL.

use anyhow::Result;

use crate::modules::session::{Fetched, HmvSession};

#[derive(Debug, Clone)]
pub struct Writeup {
    pub author: String,
    pub url: String,
}

pub struct WriteupManager {
    session: HmvSession,
}

impl WriteupManager {
    pub fn new(session: HmvSession) -> Self {
        Self { session }
    }

    /// Network-only fetch of the community writeups for a VM. Empty list =
    /// the machine exists but has no accepted writeups. Callers render.
    pub async fn fetch(&self, vm_name: &str) -> Result<Vec<Writeup>> {
        let fetched = self.session.get(&format!("/machine/{vm_name}")).await?;
        if fetched.status == 404 {
            return Err(crate::modules::HmvError::MachineNotFound(vm_name.to_string()).into());
        }
        Ok(parse_writeups(&fetched.text))
    }

    /// Network-only writeup submission. The server only serves the form after
    /// the machine is pwned and no writeup was submitted yet.
    pub async fn submit(
        &self,
        vm_name: &str,
        url: &str,
        kind: &str,
        language: &str,
    ) -> Result<UploadVerdict> {
        let fetched = self
            .session
            .post_form(
                &format!("/writeup/{vm_name}"),
                &[("type", kind), ("language", language), ("url", url.trim())],
            )
            .await?;
        Ok(classify_upload(&fetched, vm_name))
    }
}

/// Server verdict for a writeup submission.
#[derive(Debug, Clone)]
pub enum UploadVerdict {
    Submitted,
    Repeated,
    Rejected(String),
    NotFound,
    Unknown(String),
}

/// Classifies the writeup POST. An accepted submission returns an empty 200
/// fragment; ineligible ones are redirected to the dashboard.
pub fn classify_upload(fetched: &Fetched, vm_name: &str) -> UploadVerdict {
    if fetched.status == 404 {
        return UploadVerdict::NotFound;
    }
    if fetched.path == "/dashboard" {
        return UploadVerdict::Repeated;
    }
    if fetched.text.contains("[-]") {
        return UploadVerdict::Rejected(crate::modules::flag::fragment_text(&fetched.text, 120));
    }
    if fetched.path == format!("/writeup/{vm_name}") && fetched.text.trim().is_empty() {
        return UploadVerdict::Submitted;
    }
    UploadVerdict::Unknown(crate::modules::flag::fragment_text(&fetched.text, 120))
}

/// Pure parsing function so it can be unit-tested against fixture HTML.
pub fn parse_writeups(html: &str) -> Vec<Writeup> {
    let doc = scraper::Html::parse_document(html);
    let row_sel = scraper::Selector::parse("a.writeup-row").unwrap();
    let author_sel = scraper::Selector::parse("div.truncate").unwrap();

    let mut writeups = Vec::new();
    for row in doc.select(&row_sel) {
        let Some(url) = row.value().attr("href") else {
            continue;
        };
        // The first truncated div holds the author; the second holds the VM
        // name and is skipped.
        let author = row
            .select(&author_sel)
            .next()
            .map(|n| n.text().collect::<String>().trim().to_string())
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| "-".into());
        writeups.push(Writeup {
            author,
            url: url.to_string(),
        });
    }
    writeups
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"
    <html><body><section>
      <h2>COMMUNITY WRITEUPS</h2>
      <a href="https://d4fl0n.github.io/posts/echo/" target="_blank"
         class="writeup-row flex min-w-0 items-center justify-between gap-3">
        <div class="flex min-w-0 items-center gap-3">
          <div class="flex h-8 w-8">▤</div>
          <div class="min-w-0">
            <div class="truncate text-xs text-slate-300">Tomash</div>
            <div class="mt-1 truncate text-[9px] text-slate-600">Echo</div>
          </div>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <span class="hidden sm:inline"> accepted </span>
          <span class="text-pink-400"> → </span>
        </div>
      </a>
    </section></body></html>"##;

    fn fetched(status: u16, path: &str, text: &str) -> Fetched {
        Fetched {
            status,
            path: path.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn parses_author_and_url() {
        let writeups = parse_writeups(FIXTURE);
        assert_eq!(writeups.len(), 1);
        assert_eq!(writeups[0].author, "Tomash");
        assert_eq!(writeups[0].url, "https://d4fl0n.github.io/posts/echo/");
    }

    #[test]
    fn classifies_upload_verdicts() {
        assert!(matches!(
            classify_upload(&fetched(200, "/writeup/Merge", ""), "Merge"),
            UploadVerdict::Submitted
        ));
        assert!(matches!(
            classify_upload(
                &fetched(
                    200,
                    "/writeup/Merge",
                    "<div class=\"text-red-400\">[-] Wrong Filetype.</div>"
                ),
                "Merge"
            ),
            UploadVerdict::Rejected(_)
        ));
        assert!(matches!(
            classify_upload(&fetched(200, "/dashboard", "<html></html>"), "Merge"),
            UploadVerdict::Repeated
        ));
        assert!(matches!(
            classify_upload(&fetched(404, "/writeup/Nope", ""), "Nope"),
            UploadVerdict::NotFound
        ));
    }
}
