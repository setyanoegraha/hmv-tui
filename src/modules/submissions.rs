//! VM submissions: the global submission queue, the submission guidelines and
//! the submit form. All endpoints require a logged-in session.
//!
//! Form fields are scraped dynamically from `/submit/machine` so the POST
//! always matches the site's current field names; a static fallback list is
//! used when parsing fails.

use anyhow::Result;
use scraper::{ElementRef, Html, Selector};

use crate::modules::session::HmvSession;

const SUBMIT_PATH: &str = "/submit/machine";
const STATUS_PATH: &str = "/submit/status";

/// One row of the submissions table. The site shows every user's submissions
/// here; "mine" = rows whose user matches the account.
#[derive(Debug, Clone)]
pub struct QueueEntry {
    /// The account that submitted the VM.
    pub user: String,
    pub name: String,
    /// ISO-8601 timestamp as printed by the site.
    pub date: String,
    /// Raw status text from the site ("PENDING", "ACCEPTED", ...).
    pub status: String,
    /// Easy / medium / hard.
    pub level: String,
}

/// One editable form field scraped from the submit form.
#[derive(Debug, Clone)]
pub struct FormField {
    /// `name=` attribute used for the POST.
    pub name: String,
    /// Human label shown in the popup.
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
    /// Pre-filled value (hidden inputs carry theirs).
    pub value: String,
}

#[derive(Debug, Clone)]
pub enum FieldKind {
    Text,
    Select(Vec<String>),
    Hidden,
}

/// Verdict of a VM submission.
#[derive(Debug, Clone)]
pub enum SubmitVerdict {
    /// Accepted and waiting in the queue.
    Submitted,
    Rejected(String),
    Unknown(String),
}

/// Static fallback matching the `/submit/machine` form.
pub fn fallback_form_fields() -> Vec<FormField> {
    let field = |name: &str, label: &str, required: bool| FormField {
        name: name.into(),
        label: label.into(),
        kind: FieldKind::Text,
        required,
        value: String::new(),
    };
    vec![
        field("name", "VM name:", true),
        field("url", "Download URL:", true),
        field("user_flag", "User flag:", true),
        field("root_flag", "Root flag:", true),
        field("tags", "Tags:", true),
        FormField {
            name: "level".into(),
            label: "Level:".into(),
            kind: FieldKind::Select(vec!["Easy".into(), "Medium".into(), "Hard".into()]),
            required: true,
            value: "Easy".into(),
        },
        field("notes", "Notes (optional):", false),
        field("writeup", "Writeup (optional):", false),
    ]
}

pub struct SubmissionManager {
    session: HmvSession,
}

impl SubmissionManager {
    pub fn new(session: HmvSession) -> Self {
        Self { session }
    }

    /// Fetches the whole submission queue.
    pub async fn fetch_queue(&self) -> Result<Vec<QueueEntry>> {
        let fetched = self.session.get(STATUS_PATH).await?;
        Ok(parse_queue(&fetched.text))
    }

    /// Fetches the submission guidelines as plain text. Network-only.
    pub async fn fetch_rules(&self) -> Result<String> {
        let fetched = self.session.get(SUBMIT_PATH).await?;
        Ok(parse_rules(&fetched.text))
    }

    /// Scrapes the submit form fields from `/submit/machine`.
    pub async fn fetch_form(&self) -> Result<Vec<FormField>> {
        let fetched = self.session.get(SUBMIT_PATH).await?;
        let fields = parse_form_fields(&fetched.text);
        if fields.is_empty() {
            Ok(fallback_form_fields())
        } else {
            Ok(fields)
        }
    }

    /// Submits the VM with the given (name, value) pairs — names must be
    /// the ones scraped from the form (or the fallback names).
    pub async fn submit(&self, fields: &[(String, String)]) -> Result<SubmitVerdict> {
        let form: Vec<(&str, &str)> = fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let fetched = self.session.post_form(SUBMIT_PATH, &form).await?;
        Ok(classify_submit(&fetched))
    }
}

/// Classifies the submit reply. Success is rendered in green; validation and
/// rejection messages are red/pink.
pub fn classify_submit(fetched: &crate::modules::session::Fetched) -> SubmitVerdict {
    if fetched.text.contains("text-green") {
        return SubmitVerdict::Submitted;
    }
    if fetched.text.contains("required")
        || fetched.text.contains("[-]")
        || fetched.text.contains("text-red")
        || fetched.text.contains("text-pink")
    {
        return SubmitVerdict::Rejected(crate::modules::flag::fragment_text(&fetched.text, 120));
    }
    SubmitVerdict::Unknown(crate::modules::flag::fragment_text(&fetched.text, 120))
}

/// Extracts the editable + hidden fields of the first `<form>` on the page.
pub fn parse_form_fields(html: &str) -> Vec<FormField> {
    let doc = Html::parse_document(html);
    let form_sel = match Selector::parse("form").ok() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let Some(form) = doc.select(&form_sel).next() else {
        return Vec::new();
    };

    let input_sel = Selector::parse("input, select, textarea").unwrap();
    let option_sel = Selector::parse("option").unwrap();
    let label_sel = Selector::parse("label").unwrap();

    let mut fields = Vec::new();
    for el in form.select(&input_sel) {
        let name = match el.value().attr("name") {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue,
        };
        let tag = el.value().name();
        let input_type = el.value().attr("type").unwrap_or("text").to_lowercase();
        if tag == "input"
            && (input_type == "submit"
                || input_type == "button"
                || input_type == "reset"
                || input_type == "image")
        {
            continue;
        }

        let (kind, value) = if input_type == "hidden" {
            (
                FieldKind::Hidden,
                el.value().attr("value").unwrap_or("").to_string(),
            )
        } else if tag == "select" {
            // Option text is sent verbatim: the server expects "Easy", not
            // "easy".
            let options: Vec<String> = el
                .select(&option_sel)
                .map(|o| o.text().collect::<String>().trim().to_string())
                .filter(|o| !o.is_empty())
                .collect();
            let selected = el
                .select(&option_sel)
                .find(|o| o.value().attr("selected").is_some())
                .map(|o| o.text().collect::<String>().trim().to_string())
                .or_else(|| options.first().cloned())
                .unwrap_or_default();
            (FieldKind::Select(options), selected)
        } else {
            (
                FieldKind::Text,
                el.value().attr("value").unwrap_or("").to_string(),
            )
        };

        // Best-effort label: the `<label for=name>` on the page, else
        // prettified field name.
        let label = doc
            .select(&label_sel)
            .find(|l| l.value().attr("for").is_some_and(|f| f == name))
            .map(|l| l.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| prettify_name(&name));

        let required = el.value().attr("required").is_some() || label.ends_with('*');

        fields.push(FormField {
            name,
            label: format!("{}:", label.trim_end_matches(':').trim_end_matches('*')),
            kind,
            required,
            value,
        });
    }
    fields
}

/// Turns snake_case / camelCase field names into "Field name" labels.
fn prettify_name(name: &str) -> String {
    name.replace('_', " ")
        .chars()
        .enumerate()
        .map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c })
        .collect::<String>()
}

/// Parses the submissions queue. Each row repeats its values in a desktop
/// grid and a mobile grid; only the first child element is read.
pub fn parse_queue(html: &str) -> Vec<QueueEntry> {
    let doc = Html::parse_document(html);
    let row_sel = Selector::parse("div.submission-row").unwrap();

    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let Some(grid) = row.child_elements().next() else {
            continue;
        };
        let cells: Vec<String> = grid.child_elements().map(|cell| normalize(&cell)).collect();
        if cells.len() < 6 {
            continue;
        }
        out.push(QueueEntry {
            user: cells[0].clone(),
            name: cells[1].clone(),
            level: cells[2].trim_start_matches('●').trim().to_string(),
            date: cells[3].clone(),
            status: cells[4].trim_start_matches('●').trim().to_string(),
        });
    }
    out
}

/// Parses the submission guidelines: the `<ul>` that follows the
/// `Submission Guidelines` heading's wrapper `div`.
pub fn parse_rules(html: &str) -> String {
    let doc = Html::parse_document(html);
    let heading_sel = Selector::parse("h2").unwrap();

    let Some(heading) = doc
        .select(&heading_sel)
        .find(|h| h.text().collect::<String>().trim() == "Submission Guidelines")
    else {
        return "Submission guidelines unavailable.".into();
    };

    // The heading sits in an icon+title div; the guideline list is that
    // div's next element sibling (whitespace text nodes in between).
    let list = heading.parent().and_then(|parent| {
        let mut node = parent.next_sibling();
        while let Some(current) = node {
            if let Some(element) = ElementRef::wrap(current) {
                return (element.value().name() == "ul").then_some(element);
            }
            node = current.next_sibling();
        }
        None
    });

    let Some(list) = list else {
        return "Submission guidelines unavailable.".into();
    };

    let lines: Vec<String> = list
        .child_elements()
        .filter(|li| li.value().name() == "li")
        .map(|li| normalize(&li.child_elements().last().unwrap_or(li)))
        .collect();
    lines.join("\n")
}

/// Collapses an element's text to a single normalized line.
fn normalize(el: &ElementRef<'_>) -> String {
    el.text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::session::Fetched;

    const QUEUE_FIXTURE: &str = r##"
    <html><body><section>
    <div class="submission-row border-b border-white/[0.045] px-5 py-5">
      <div class="hidden items-center md:grid">
        <div class="min-w-0 truncate text-sm text-zinc-300">chinese</div>
        <div class="min-w-0 truncate text-sm text-pink-300">Chinese1</div>
        <div class="text-xs"><span class="text-green-400">● Easy</span></div>
        <div class="text-xs text-zinc-600">2026-11-01T09:53:12Z</div>
        <div class="text-xs"><span class="text-yellow-400">● PENDING</span></div>
        <div class="min-w-0 text-xs reason"></div>
      </div>
      <div class="md:hidden"><div>chinese</div><div>Chinese1</div></div>
    </div>
    </section></body></html>"##;

    const FORM_FIXTURE: &str = r##"
    <html><body>
    <form action="/submit/machine" method="POST">
      <label for="name">Name</label>
      <input id="name" name="name" type="text" maxlength="32">
      <label for="url">URL</label>
      <input id="url" name="url" type="url" maxlength="255">
      <label for="user_flag">User Flag</label>
      <input id="user_flag" name="user_flag" type="text">
      <label for="root_flag">Root Flag</label>
      <input id="root_flag" name="root_flag" type="text">
      <label for="tags">Tags</label>
      <input id="tags" name="tags" type="text" maxlength="120">
      <label for="level">Level</label>
      <select id="level" name="level">
        <option value="Easy">Easy</option>
        <option value="Medium">Medium</option>
        <option value="Hard">Hard</option>
      </select>
      <label for="notes">Notes</label>
      <textarea id="notes" name="notes" rows="4" maxlength="500"></textarea>
      <label for="writeup">Writeup</label>
      <textarea id="writeup" name="writeup" rows="4" maxlength="500"></textarea>
      <button type="submit">Submit VM</button>
    </form>
    </body></html>"##;

    const RULES_FIXTURE: &str = r#"
    <html><body><div>
      <div class="mb-6 flex items-center gap-3">
        <svg><path d="M12 3"/></svg>
        <h2 class="text-xs font-semibold">Submission Guidelines</h2>
      </div>
      <ul class="space-y-4">
        <li class="flex gap-3"><span class="text-pink-400">✓</span><span>The VM must be based on your own work.</span></li>
        <li class="flex gap-3"><span class="text-pink-400">✓</span><span>Do not include illegal content or real data.</span></li>
        <li class="flex gap-3"><span class="text-pink-400">✓</span><span>Ensure the VM is solvable and not brute-force only.</span></li>
        <li class="flex gap-3"><span class="text-pink-400">✓</span><span>Test the VM before submitting.</span></li>
        <li class="flex gap-3"><span class="text-pink-400">✓</span><span>Avoid bruteforce that takes more than 5 minutes.</span></li>
      </ul>
      <div><h3>Good tags examples</h3></div>
    </div></body></html>"#;

    fn fetched(text: &str) -> Fetched {
        Fetched {
            status: 200,
            path: "/submit/machine".into(),
            text: text.into(),
        }
    }

    #[test]
    fn parses_queue_six_cells() {
        let queue = parse_queue(QUEUE_FIXTURE);
        assert_eq!(queue.len(), 1, "mobile duplicate grid must not be counted");
        assert_eq!(queue[0].user, "chinese");
        assert_eq!(queue[0].name, "Chinese1");
        assert_eq!(queue[0].level, "Easy");
        assert_eq!(queue[0].date, "2026-11-01T09:53:12Z");
        assert_eq!(queue[0].status, "PENDING");
    }

    #[test]
    fn parses_rules_from_guidelines_list() {
        let rules = parse_rules(RULES_FIXTURE);
        let lines: Vec<&str> = rules.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "The VM must be based on your own work.");
        assert!(
            rules.contains("Avoid bruteforce"),
            "last rule missing: {rules}"
        );
        assert!(!rules.contains('✓'), "glyph must be dropped: {rules}");
    }

    #[test]
    fn parse_rules_without_heading_degrades() {
        assert_eq!(
            parse_rules("<html></html>"),
            "Submission guidelines unavailable."
        );
    }

    #[test]
    fn parses_form_fields_preserving_level_case() {
        let fields = parse_form_fields(FORM_FIXTURE);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "name",
                "url",
                "user_flag",
                "root_flag",
                "tags",
                "level",
                "notes",
                "writeup"
            ]
        );
        assert!(!names.contains(&"submit"), "submit button excluded");

        let level = fields.iter().find(|f| f.name == "level").unwrap();
        match &level.kind {
            FieldKind::Select(opts) => assert_eq!(opts, &vec!["Easy", "Medium", "Hard"]),
            other => panic!("level should be a select, got {other:?}"),
        }
        assert_eq!(level.value, "Easy");
        assert!(
            fields.iter().all(|f| !f.required),
            "page has no required attrs"
        );
    }

    #[test]
    fn fallback_fields_cover_owner_requirements() {
        let fields = fallback_form_fields();
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "name",
                "url",
                "user_flag",
                "root_flag",
                "tags",
                "level",
                "notes",
                "writeup"
            ]
        );
        assert!(!fields[6].required, "notes optional");
        assert!(!fields[7].required, "writeup optional");
        assert!(fields[..6].iter().all(|f| f.required));
        match &fields[5].kind {
            FieldKind::Select(opts) => assert_eq!(opts, &vec!["Easy", "Medium", "Hard"]),
            other => panic!("level should be a select, got {other:?}"),
        }
        assert_eq!(fields[5].value, "Easy");
    }

    #[test]
    fn classifies_submit_replies() {
        assert!(matches!(
            classify_submit(&fetched("<div class=\"text-green-400\">Queued</div>")),
            SubmitVerdict::Submitted
        ));
        assert!(matches!(
            classify_submit(&fetched(
                "<div class=\"text-pink-400\">All fields are required</div>"
            )),
            SubmitVerdict::Rejected(_)
        ));
        assert!(matches!(
            classify_submit(&fetched("<div>something else</div>")),
            SubmitVerdict::Unknown(_)
        ));
    }
}
