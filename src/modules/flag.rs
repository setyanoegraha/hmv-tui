//! Flag submission for a specific VM.

use anyhow::Result;

use crate::modules::session::{Fetched, HmvSession};

/// Server verdict for a flag check, decoupled from rendering so the TUI can
/// present it in its result popups. The endpoint takes one flag and infers
/// whether it is the user or the root flag.
#[derive(Debug, Clone)]
pub enum FlagVerdict {
    Accepted,
    Wrong,
    AlreadyPwned,
    MachineNotFound,
    Unknown(String),
}

pub struct FlagManager {
    session: HmvSession,
}

impl FlagManager {
    pub fn new(session: HmvSession) -> Self {
        Self { session }
    }

    /// Network-only check: submits the flag and classifies the response.
    pub async fn check(&self, vm: &str, flag: &str) -> Result<FlagVerdict> {
        let fetched = self
            .session
            .post_form(&format!("/machine/{vm}"), &[("flag", flag)])
            .await?;
        Ok(classify_flag(&fetched))
    }
}

/// Classifies the reply fragment. Every rejection is rendered with `[-]`;
/// a correct flag yields a non-`[-]` fragment.
pub fn classify_flag(fetched: &Fetched) -> FlagVerdict {
    if fetched.status == 404 {
        return FlagVerdict::MachineNotFound;
    }
    let text = fragment_text(&fetched.text, 400);
    if text.contains("ALREADY PWNED") {
        FlagVerdict::AlreadyPwned
    } else if text.contains("WRONG FLAG") || text.contains("EMPTY FLAG") {
        FlagVerdict::Wrong
    } else if fetched.text.contains("[-]") {
        FlagVerdict::Unknown(text)
    } else {
        FlagVerdict::Accepted
    }
}

/// Body fragment → plain text: drops tags, collapses whitespace, caps length.
pub fn fragment_text(html: &str, max: usize) -> String {
    let doc = scraper::Html::parse_fragment(html);
    let raw = doc.root_element().text().collect::<String>();
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetched(status: u16, path: &str, text: &str) -> Fetched {
        Fetched {
            status,
            path: path.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn classifies_reply_fragments() {
        let wrong = r#"<div class="text-red-400">[-] WRONG FLAG</div>"#;
        assert!(matches!(
            classify_flag(&fetched(200, "/machine/Bitb", wrong)),
            FlagVerdict::Wrong
        ));

        let already = r#"<div class="text-red-400">[-] VM ALREADY PWNED.</div>"#;
        assert!(matches!(
            classify_flag(&fetched(200, "/machine/Gift", already)),
            FlagVerdict::AlreadyPwned
        ));

        let empty = r#"<div class="text-red-400">[-] EMPTY FLAG</div>"#;
        assert!(matches!(
            classify_flag(&fetched(200, "/machine/Bitb", empty)),
            FlagVerdict::Wrong
        ));

        let ok = r#"<div class="text-green-400">CONGRATULATIONS</div>"#;
        assert!(matches!(
            classify_flag(&fetched(200, "/machine/Bitb", ok)),
            FlagVerdict::Accepted
        ));

        assert!(matches!(
            classify_flag(&fetched(404, "/machine/Nope", "<html>404</html>")),
            FlagVerdict::MachineNotFound
        ));
    }

    #[test]
    fn fragment_text_strips_tags_and_collapses() {
        let html = "<div class=\"text-red-400\">\n  [-] WRONG\n  FLAG\n</div>";
        assert_eq!(fragment_text(html, 120), "[-] WRONG FLAG");
        assert_eq!(fragment_text("<b>abcdef</b>", 3), "abc");
    }
}
