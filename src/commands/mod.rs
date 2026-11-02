//! Dashboard orchestration: session lifecycle, data fetching and the host
//! closures handed to the TUI event loop.

use anyhow::Result;

use crate::config::ConfigManager;
use crate::modules::flag::FlagManager;
use crate::modules::machines::MachineScraper;
use crate::modules::releases::ReleaseScraper;
use crate::modules::session::{login, login_with, HmvSession};
use crate::modules::writeups::WriteupManager;
use crate::modules::HmvError;
use crate::tui::{ActionReport, TuiAction, TuiData};

/// Reusable authenticated session for the TUI's lifetime. Cloning an
/// `HmvSession` is cheap (shared connection pool), so one login serves
/// every background action.
#[derive(Clone)]
pub struct SessionCache {
    session: HmvSession,
    username: String,
}

impl SessionCache {
    pub async fn new() -> Result<Self> {
        let cfg = ConfigManager::new();
        let (username, _) = cfg.load_credentials()?;
        let session = login(&cfg).await?;
        Ok(Self { session, username })
    }

    pub fn session(&self) -> HmvSession {
        self.session.clone()
    }

    pub fn username(&self) -> &str {
        &self.username
    }
}

/// Session slot shared by all TUI closures; `None` until the config popup
/// succeeds and after a logout.
type SharedSession = std::sync::Arc<std::sync::Mutex<Option<SessionCache>>>;

fn take_session(shared: &SharedSession) -> Result<SessionCache> {
    shared
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Not configured — no HackMyVM session available."))
}

pub async fn tui_cmd() -> Result<()> {
    // Bare `hmv`: without usable stored credentials the TUI starts directly
    // in the config popup; otherwise log in now and enter the dashboard with
    // an empty state (the first fetch runs inside the event loop with a
    // `⟳ Loading data...` indicator).
    let cfg = ConfigManager::new();
    let stored_username = cfg.stored_username();
    let sessions = if stored_username.is_some() {
        match SessionCache::new().await {
            Ok(sessions) => Some(sessions),
            Err(error)
                if error.chain().any(|cause| {
                    cause
                        .downcast_ref::<HmvError>()
                        .is_some_and(|e| matches!(e, HmvError::AuthFailed))
                }) =>
            {
                // Stale password: offer re-configuration inside the TUI.
                None
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    let unconfigured = sessions.is_none();

    let shared: SharedSession = std::sync::Arc::new(std::sync::Mutex::new(sessions));
    let fetch_sessions = shared.clone();
    let action_sessions = shared.clone();
    let writeups_sessions = shared.clone();
    let config_sessions = shared.clone();
    let logout_sessions = shared.clone();
    let submissions_sessions = shared.clone();
    let lang = cfg.language();
    let initial = if unconfigured {
        crate::tui::AppState::unconfigured_with_lang(stored_username.as_deref(), lang)
    } else {
        crate::tui::AppState::loading_with_lang(lang)
    };

    // The closures now run on background threads inside the TUI, so grab
    // the multi-thread runtime handle here (we are inside #[tokio::main])
    // and block on it from those threads — no block_in_place needed.
    let handle = tokio::runtime::Handle::current();
    crate::tui::run(
        initial,
        {
            let handle = handle.clone();
            move || {
                let sessions = take_session(&fetch_sessions)?;
                handle.block_on(fetch_tui_data(&sessions))
            }
        },
        {
            let handle = handle.clone();
            move |action| {
                let sessions = take_session(&action_sessions)?;
                handle.block_on(run_tui_action(&sessions, action))
            }
        },
        {
            let handle = handle.clone();
            move |vm| {
                let sessions = take_session(&writeups_sessions)?;
                handle.block_on(async { WriteupManager::new(sessions.session()).fetch(vm).await })
            }
        },
        {
            let handle = handle.clone();
            move |username, password| {
                handle.block_on(configure_account(&config_sessions, username, password))
            }
        },
        {
            let handle = handle.clone();
            move || handle.block_on(logout_account(&logout_sessions))
        },
        {
            let handle = handle.clone();
            move || {
                let sessions = take_session(&submissions_sessions)?;
                handle.block_on(async {
                    let manager =
                        crate::modules::submissions::SubmissionManager::new(sessions.session());
                    let queue = manager.fetch_queue().await.unwrap_or_default();
                    let rules = manager.fetch_rules().await.ok();
                    let form = manager.fetch_form().await.unwrap_or_default();
                    Ok((queue, rules, form))
                })
            }
        },
    )
}

/// Validates the entered credentials by logging in, stores them, and
/// installs the session for the rest of the dashboard lifetime. Used by the
/// first-run popup and by account switching.
async fn configure_account(shared: &SharedSession, username: &str, password: &str) -> Result<()> {
    let session = login_with(username, password).await?;
    ConfigManager::new().save_credentials(username, password)?;
    *shared.lock().unwrap() = Some(SessionCache {
        session,
        username: username.to_string(),
    });
    Ok(())
}

/// Removes the stored account and drops the in-memory session. Called from
/// the account popup (`l`). Running downloads are unaffected — they use
/// public MEGA links, not the session.
async fn logout_account(shared: &SharedSession) -> Result<()> {
    ConfigManager::new().clear_credentials()?;
    *shared.lock().unwrap() = None;
    Ok(())
}

/// Executes a user action from a TUI popup. Returns the result popup
/// content: verdicts labeled with the original field (User/Root flag),
/// `changed` telling whether dashboard data must be refreshed after the
/// popup closes.
async fn run_tui_action(sessions: &SessionCache, action: TuiAction) -> Result<ActionReport> {
    if matches!(
        action.kind,
        crate::tui::PopupKind::Download
            | crate::tui::PopupKind::Config
            | crate::tui::PopupKind::Account
            | crate::tui::PopupKind::Rules
    ) {
        anyhow::bail!("downloads, configuration and logout are handled directly by the event loop");
    }
    match action.kind {
        crate::tui::PopupKind::Download
        | crate::tui::PopupKind::Config
        | crate::tui::PopupKind::Account
        | crate::tui::PopupKind::Rules => unreachable!("handled by the event loop"),
        crate::tui::PopupKind::SubmissionForm => {
            use crate::i18n::Key;
            use crate::tui::{ActionReport, ReportKind};
            let lang = crate::config::ConfigManager::new().language();
            let manager = crate::modules::submissions::SubmissionManager::new(sessions.session());
            let verdict = manager.submit(&action.values).await?;
            let (entries, changed, status) = match verdict {
                crate::modules::submissions::SubmitVerdict::Submitted => (
                    vec![(ReportKind::Success, lang.t(Key::SubmitOk).to_string())],
                    true,
                    lang.t(Key::SubmitOk).to_string(),
                ),
                crate::modules::submissions::SubmitVerdict::Rejected(reason) => (
                    vec![(
                        ReportKind::Failure,
                        crate::tui::fmt_key(lang.t(Key::SubmitFailed), &[&reason]),
                    )],
                    false,
                    crate::tui::fmt_key(lang.t(Key::SubmitFailed), &[&reason]),
                ),
                crate::modules::submissions::SubmitVerdict::Unknown(body) => (
                    vec![(
                        ReportKind::Info,
                        crate::tui::fmt_key(lang.t(Key::SubmitUnknown), &[&body]),
                    )],
                    false,
                    crate::tui::fmt_key(lang.t(Key::SubmitUnknown), &[&body]),
                ),
            };
            Ok(ActionReport {
                title: crate::tui::fmt_key(lang.t(Key::SubmitFormTitle), &[&action.vm]),
                entries,
                changed,
                status,
            })
        }
        crate::tui::PopupKind::Flag => {
            let Some((_, flag)) = action.values.first() else {
                anyhow::bail!("No flag entered.");
            };
            let verdict = FlagManager::new(sessions.session())
                .check(&action.vm, flag)
                .await?;
            let lang = crate::config::ConfigManager::new().language();
            Ok(crate::tui::build_flag_report(lang, &action.vm, verdict))
        }
        crate::tui::PopupKind::Upload => {
            let value = |name: &str| -> String {
                action
                    .values
                    .iter()
                    .find(|(field, _)| field == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            let url = value("url");
            let kind = value("type");
            let language = value("language");
            let lang = crate::config::ConfigManager::new().language();
            use crate::tui::{ActionReport, ReportKind};
            let verdict = WriteupManager::new(sessions.session())
                .submit(&action.vm, &url, &kind, &language)
                .await?;
            use crate::modules::writeups::UploadVerdict;
            let (entries, changed, status) = match verdict {
                UploadVerdict::Submitted => (
                    vec![(
                        ReportKind::Success,
                        crate::tui::fmt_key(
                            lang.t(crate::i18n::Key::ReportWriteupAccepted),
                            &[&url],
                        ),
                    )],
                    true,
                    format!("[✓] Writeup submitted for {}!", action.vm),
                ),
                UploadVerdict::Repeated => (
                    vec![(
                        ReportKind::Info,
                        lang.t(crate::i18n::Key::ReportWriteupRepeated).to_string(),
                    )],
                    false,
                    format!("[=] Writeup for {} was already submitted.", action.vm),
                ),
                UploadVerdict::Rejected(reason) => (
                    vec![(
                        ReportKind::Failure,
                        crate::tui::fmt_key(
                            lang.t(crate::i18n::Key::ReportWriteupRejected),
                            &[&reason],
                        ),
                    )],
                    false,
                    format!("[!] Server rejected writeup for {}.", action.vm),
                ),
                UploadVerdict::NotFound => (
                    vec![(
                        ReportKind::Failure,
                        crate::tui::fmt_key(
                            lang.t(crate::i18n::Key::ReportMachineNotFound),
                            &[&action.vm],
                        ),
                    )],
                    false,
                    format!("[!] Machine '{}' not found.", action.vm),
                ),
                UploadVerdict::Unknown(body) => (
                    vec![(
                        ReportKind::Info,
                        crate::tui::fmt_key(
                            lang.t(crate::i18n::Key::ReportUnknownResponse),
                            &[&body],
                        ),
                    )],
                    false,
                    format!("[?] Unknown response: {body}"),
                ),
            };
            Ok(ActionReport {
                title: crate::tui::fmt_key(
                    lang.t(crate::i18n::Key::WriteupResultsTitle),
                    &[&action.vm],
                ),
                entries,
                changed,
                status,
            })
        }
    }
}

/// Fetches every dataset the dashboard shows: the machine catalog (which
/// carries per-row pwned state), the release schedule and the submission
/// queue/form/guidelines. No terminal output here — the TUI owns the screen
/// and reports progress through its footer.
async fn fetch_tui_data(sessions: &SessionCache) -> Result<TuiData> {
    let session = sessions.session();

    let catalog = MachineScraper::new(session.clone()).get_catalog().await?;

    let total_vms = catalog.len() as u64;
    let pwned_vms = catalog.iter().filter(|m| m.status == "PWNED").count() as u64;

    let count = |pred: &dyn Fn(&crate::modules::machines::Machine) -> bool| -> (u64, u64) {
        let matching: Vec<&crate::modules::machines::Machine> =
            catalog.iter().filter(|m| pred(m)).collect();
        let pwned = matching.iter().filter(|m| m.status == "PWNED").count() as u64;
        (pwned, matching.len() as u64)
    };
    let level = |name: &str| {
        let needle = name.to_string();
        count(&move |m: &crate::modules::machines::Machine| {
            m.difficulty.eq_ignore_ascii_case(&needle)
        })
    };
    let os = |name: &str| {
        let needle = name.to_string();
        count(&move |m: &crate::modules::machines::Machine| m.os.eq_ignore_ascii_case(&needle))
    };

    // Release schedule is nice-to-have: a failure here must not blank out
    // the whole dashboard, so it degrades to an empty tab.
    let releases = ReleaseScraper::new(session.clone())
        .get_releases()
        .await
        .unwrap_or_default();

    // Submission queue + guidelines are nice-to-have: failures degrade to an
    // empty tab instead of blanking out the dashboard.
    let submissions = crate::modules::submissions::SubmissionManager::new(session.clone());
    let queue = submissions.fetch_queue().await.unwrap_or_default();
    let rules = submissions.fetch_rules().await.ok();
    let form = submissions.fetch_form().await.unwrap_or_default();

    Ok(TuiData {
        username: sessions.username().to_string(),
        progress: vec![
            ("Total VMs".to_string(), pwned_vms, total_vms),
            ("Easy".to_string(), level("Easy").0, level("Easy").1),
            ("Medium".to_string(), level("Medium").0, level("Medium").1),
            ("Hard".to_string(), level("Hard").0, level("Hard").1),
            ("Linux".to_string(), os("Linux").0, os("Linux").1),
            ("Windows".to_string(), os("Windows").0, os("Windows").1),
        ],
        catalog,
        releases,
        submissions: queue,
        submission_form: form,
        rules,
    })
}
