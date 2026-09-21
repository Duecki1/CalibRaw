use super::*;

const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/Duecki1/CalibRaw/releases/latest";
const GITHUB_RELEASE_URL_PREFIX: &str = "https://github.com/Duecki1/CalibRaw/releases/";
const GITHUB_PRIVACY_URL: &str =
    "https://docs.github.com/de/site-policy/privacy-policies/github-general-privacy-statement";
const MAX_RELEASE_RESPONSE_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug)]
struct AvailableUpdate {
    version: semver::Version,
    tag: String,
    name: Option<String>,
    url: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    html_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VersionCheckConsentRequest {
    Startup,
    EnableAutomatic,
    Manual,
    Settings,
}

#[derive(Clone, Debug)]
enum VersionCheckStatus {
    NotChecked,
    Checking,
    NoPublishedRelease,
    UpToDate,
    Available {
        version: semver::Version,
        ignored: bool,
    },
    Failed(String),
}

pub(in crate::app) struct VersionCheckState {
    receiver: Option<mpsc::Receiver<Result<Option<AvailableUpdate>, String>>>,
    requested_manually: bool,
    consent_dialog: Option<VersionCheckConsentRequest>,
    dialog: Option<AvailableUpdate>,
    status: VersionCheckStatus,
}

impl Default for VersionCheckState {
    fn default() -> Self {
        Self {
            receiver: None,
            requested_manually: false,
            consent_dialog: None,
            dialog: None,
            status: VersionCheckStatus::NotChecked,
        }
    }
}

impl VersionCheckState {
    pub(super) fn dialog_open(&self) -> bool {
        self.consent_dialog.is_some() || self.dialog.is_some()
    }
}

fn normalized_version(tag: &str) -> Result<semver::Version, String> {
    let trimmed = tag.trim();
    let version = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    semver::Version::parse(version)
        .map_err(|error| format!("GitHub returned an invalid release version {tag:?}: {error}"))
}

const fn should_show_update(requested_manually: bool, auto_check: bool, ignored: bool) -> bool {
    !ignored && (requested_manually || auto_check)
}

const fn github_version_check_permitted(permission: Option<bool>) -> bool {
    matches!(permission, Some(true))
}

fn fetch_latest_release() -> Result<Option<AvailableUpdate>, String> {
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(12)))
        .build();
    let agent: ureq::Agent = config.into();
    let response = agent
        .get(GITHUB_LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .header(
            "User-Agent",
            concat!("CalibRaw/", env!("CARGO_PKG_VERSION")),
        )
        .call();
    let mut response = match response {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(error) => return Err(format!("GitHub update request failed: {error}")),
    };
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_RELEASE_RESPONSE_BYTES)
        .read_to_string()
        .map_err(|error| format!("Could not read GitHub's update response: {error}"))?;
    let release: GithubRelease = serde_json::from_str(&body)
        .map_err(|error| format!("Could not understand GitHub's update response: {error}"))?;
    if !release.html_url.starts_with(GITHUB_RELEASE_URL_PREFIX) {
        return Err("GitHub returned an unexpected release URL".to_owned());
    }
    let version = normalized_version(&release.tag_name)?;
    Ok(Some(AvailableUpdate {
        version,
        tag: release.tag_name,
        name: release.name,
        url: release.html_url,
    }))
}

impl CalibRawApp {
    fn start_version_check(&mut self, requested_manually: bool) {
        // This is the final gate before any GitHub network request. Keep it here even
        // though callers also check permission, so future call sites cannot bypass a
        // saved denial accidentally.
        if !github_version_check_permitted(self.preferences.github_update_check_allowed) {
            return;
        }
        if self.ui.version_check.receiver.is_some() {
            return;
        }

        let (sender, receiver) = mpsc::channel();
        let repaint = self.egui_ctx.clone();
        match std::thread::Builder::new()
            .name("calibraw-version-check".to_owned())
            .spawn(move || {
                let result = fetch_latest_release();
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(_) => {
                self.ui.version_check.receiver = Some(receiver);
                self.ui.version_check.requested_manually = requested_manually;
                self.ui.version_check.status = VersionCheckStatus::Checking;
            }
            Err(error) => {
                self.ui.version_check.status = VersionCheckStatus::Failed(format!(
                    "Could not start the update checker: {error}"
                ));
            }
        }
    }

    pub(crate) fn check_for_updates(&mut self, requested_manually: bool) {
        if self.ui.version_check.receiver.is_some() {
            return;
        }
        if github_version_check_permitted(self.preferences.github_update_check_allowed) {
            self.start_version_check(requested_manually);
            return;
        }

        if requested_manually && self.preferences.github_update_check_allowed.is_none() {
            self.ui.version_check.consent_dialog = Some(VersionCheckConsentRequest::Manual);
        } else if self.preferences.auto_check_updates
            && self.preferences.github_update_check_allowed.is_none()
        {
            self.ui.version_check.consent_dialog = Some(VersionCheckConsentRequest::Startup);
        }
    }

    pub(crate) fn set_auto_check_updates(&mut self, enabled: bool) {
        if !enabled {
            if !self.preferences.auto_check_updates {
                return;
            }
            self.preferences.auto_check_updates = false;
            self.persist_performance_settings();
            return;
        }

        match self.preferences.github_update_check_allowed {
            Some(true) => {
                if !self.preferences.auto_check_updates {
                    self.preferences.auto_check_updates = true;
                    self.persist_performance_settings();
                }
                self.start_version_check(false);
            }
            Some(false) => {
                // A saved denial is sticky. Enabling update checks cannot override it;
                // the user must explicitly change the GitHub permission in Settings.
                self.preferences.auto_check_updates = false;
                self.persist_performance_settings();
            }
            None => {
                self.ui.version_check.consent_dialog =
                    Some(VersionCheckConsentRequest::EnableAutomatic);
            }
        }
    }

    pub(crate) fn version_check_permission_granted(&self) -> bool {
        github_version_check_permitted(self.preferences.github_update_check_allowed)
    }

    pub(crate) fn version_check_permission_denied(&self) -> bool {
        self.preferences.github_update_check_allowed == Some(false)
    }

    pub(crate) fn version_check_permission_text(&self) -> &'static str {
        match self.preferences.github_update_check_allowed {
            Some(true) => "Allowed. GitHub version-check connections may be made.",
            Some(false) => {
                "Not allowed. GitHub version checks are blocked until you explicitly allow them in Settings."
            }
            None => "Not decided. CalibRaw will ask before contacting GitHub.",
        }
    }

    pub(crate) fn review_version_check_privacy(&mut self) {
        self.ui.version_check.consent_dialog = Some(VersionCheckConsentRequest::Settings);
    }

    pub(crate) fn revoke_version_check_permission(&mut self) {
        self.preferences.github_update_check_allowed = Some(false);
        self.preferences.auto_check_updates = false;
        self.ui.version_check.receiver = None;
        self.ui.version_check.consent_dialog = None;
        self.ui.version_check.dialog = None;
        self.ui.version_check.status = VersionCheckStatus::NotChecked;
        self.persist_performance_settings();
    }

    pub(crate) fn version_check_in_progress(&self) -> bool {
        self.ui.version_check.receiver.is_some()
    }

    pub(crate) fn version_check_status_text(&self) -> String {
        match &self.ui.version_check.status {
            VersionCheckStatus::NotChecked => "Not checked during this session.".to_owned(),
            VersionCheckStatus::Checking => "Checking GitHub…".to_owned(),
            VersionCheckStatus::NoPublishedRelease => {
                "No published GitHub release is available yet.".to_owned()
            }
            VersionCheckStatus::UpToDate => {
                format!("CalibRaw {} is up to date.", env!("CARGO_PKG_VERSION"))
            }
            VersionCheckStatus::Available { version, ignored } => {
                if *ignored {
                    format!("Version {version} is available and ignored.")
                } else {
                    format!("Version {version} is available.")
                }
            }
            VersionCheckStatus::Failed(error) => error.clone(),
        }
    }

    pub(in crate::app) fn poll_version_check(&mut self) {
        let update = self
            .ui
            .version_check
            .receiver
            .as_ref()
            .map(mpsc::Receiver::try_recv);
        let result = match update {
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                Err("The GitHub update checker stopped unexpectedly.".to_owned())
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => return,
        };
        self.ui.version_check.receiver = None;

        match result {
            Ok(None) => {
                self.ui.version_check.status = VersionCheckStatus::NoPublishedRelease;
            }
            Ok(Some(release)) => {
                let current = match semver::Version::parse(env!("CARGO_PKG_VERSION")) {
                    Ok(version) => version,
                    Err(error) => {
                        self.ui.version_check.status = VersionCheckStatus::Failed(format!(
                            "This build has an invalid version: {error}"
                        ));
                        return;
                    }
                };
                if release.version <= current {
                    self.ui.version_check.status = VersionCheckStatus::UpToDate;
                    return;
                }

                let ignored = self.preferences.ignored_update_version.as_deref()
                    == Some(release.version.to_string().as_str());
                self.ui.version_check.status = VersionCheckStatus::Available {
                    version: release.version.clone(),
                    ignored,
                };
                let should_open = should_show_update(
                    self.ui.version_check.requested_manually,
                    self.preferences.auto_check_updates,
                    ignored,
                );
                if should_open {
                    self.ui.version_check.dialog = Some(release);
                }
            }
            Err(error) => {
                crate::diagnostics::record(format!("Version check failed: {error}"));
                self.ui.version_check.status = VersionCheckStatus::Failed(error);
            }
        }
    }

    pub(in crate::app) fn show_version_check_consent_dialog(&mut self, ctx: &egui::Context) {
        if self.ui.onboarding_step.is_some() {
            return;
        }
        let Some(request) = self.ui.version_check.consent_dialog else {
            return;
        };

        #[derive(Clone, Copy)]
        enum ConsentAction {
            Dismiss,
            Deny,
            Allow,
        }

        let available = ctx.content_rect().size() - egui::vec2(32.0, 32.0);
        let width = available
            .x
            .clamp(1.0, crate::ui::theme::DIALOG_WIDTH_LARGE);
        let max_body_height = (available.y - crate::ui::theme::CONTROL_HEIGHT - 48.0).max(1.0);
        let mut action = None;

        egui::Modal::new(egui::Id::new("calibraw-github-version-check-consent")).show(ctx, |ui| {
            ui.set_width(width);
            ui.set_max_width(width);
            egui::ScrollArea::vertical()
                .max_height(max_body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.heading("Privacy / Datenschutz");
                    ui.add_space(crate::ui::theme::SPACE_XS);
                    ui.strong("GitHub version checks");
                    ui.label(
                        "CalibRaw can contact GitHub's Releases API to see whether a newer stable version is available. This is optional and is not required to edit or export photos.",
                    );

                    ui.add_space(crate::ui::theme::SPACE_SM);
                    ui.strong("What is sent to GitHub");
                    ui.label(format!(
                        "• The public IP address used for the connection and normal HTTPS connection/request metadata.\n\
                         • A User-Agent identifying CalibRaw and its installed version: CalibRaw/{}.\n\
                         • A request for the latest CalibRaw release metadata.\n\n\
                         CalibRaw does not send photos, file paths, project data, a CalibRaw account identifier, or analytics/telemetry as part of this check.",
                        env!("CARGO_PKG_VERSION")
                    ));

                    ui.add_space(crate::ui::theme::SPACE_SM);
                    ui.strong("How the check works");
                    ui.label(
                        "If automatic checks are enabled, CalibRaw checks once when the app starts. It does not download or install an update automatically. The response is used only to compare the published version with the installed version.",
                    );
                    ui.label(
                        "GitHub processes connection and service-usage data under its own privacy statement. CalibRaw stores your permission choice, the automatic-check setting, and any ignored release version locally in its settings.",
                    );

                    ui.add_space(crate::ui::theme::SPACE_SM);
                    ui.strong("Your choice");
                    ui.label(
                        "Permission is optional. If you choose Don't allow, automatic and manual GitHub version checks stay blocked until you explicitly change this permission under Settings → Updates. You can also revoke an existing permission there. Revoking stops future requests; it cannot undo a request that has already been made.",
                    );
                    ui.hyperlink_to(
                        "GitHub privacy statement / Datenschutzerklärung",
                        GITHUB_PRIVACY_URL,
                    );

                    ui.add_space(crate::ui::theme::SPACE_SM);
                    match request {
                        VersionCheckConsentRequest::Startup => {
                            ui.small(
                                "Choose Not now to keep using CalibRaw without automatic checks, or allow the optional startup check.",
                            );
                        }
                        VersionCheckConsentRequest::EnableAutomatic => {
                            ui.small(
                                "Automatic checking will only be enabled if you allow GitHub version-check connections.",
                            );
                        }
                        VersionCheckConsentRequest::Manual => {
                            ui.small(
                                "To check now, allow GitHub version checks. If you decline, CalibRaw will not contact GitHub again unless you explicitly change this permission in Settings.",
                            );
                        }
                        VersionCheckConsentRequest::Settings => {
                            ui.small(format!(
                                "Current permission: {}",
                                self.version_check_permission_text()
                            ));
                        }
                    }
                });

            crate::ui::theme::dialog_button_row(ui, |ui| match request {
                VersionCheckConsentRequest::Startup => {
                    if crate::ui::theme::secondary_button(ui, "Not now").clicked() {
                        action = Some(ConsentAction::Dismiss);
                    }
                    if crate::ui::theme::secondary_button(ui, "Don't allow").clicked() {
                        action = Some(ConsentAction::Deny);
                    }
                    if crate::ui::theme::primary_action_button(ui, "Allow automatic checks")
                        .clicked()
                    {
                        action = Some(ConsentAction::Allow);
                    }
                }
                VersionCheckConsentRequest::EnableAutomatic => {
                    if crate::ui::theme::secondary_button(ui, "Cancel").clicked() {
                        action = Some(ConsentAction::Dismiss);
                    }
                    if crate::ui::theme::secondary_button(ui, "Don't allow").clicked() {
                        action = Some(ConsentAction::Deny);
                    }
                    if crate::ui::theme::primary_action_button(ui, "Allow automatic checks")
                        .clicked()
                    {
                        action = Some(ConsentAction::Allow);
                    }
                }
                VersionCheckConsentRequest::Manual => {
                    if crate::ui::theme::secondary_button(ui, "Cancel").clicked() {
                        action = Some(ConsentAction::Dismiss);
                    }
                    if crate::ui::theme::secondary_button(ui, "Don't allow").clicked() {
                        action = Some(ConsentAction::Deny);
                    }
                    if crate::ui::theme::primary_action_button(ui, "Allow & check").clicked() {
                        action = Some(ConsentAction::Allow);
                    }
                }
                VersionCheckConsentRequest::Settings => {
                    if crate::ui::theme::secondary_button(ui, "Cancel").clicked() {
                        action = Some(ConsentAction::Dismiss);
                    }
                    if crate::ui::theme::secondary_button(ui, "Don't allow").clicked() {
                        action = Some(ConsentAction::Deny);
                    }
                    if crate::ui::theme::primary_action_button(ui, "Allow & remember").clicked() {
                        action = Some(ConsentAction::Allow);
                    }
                }
            });
        });

        match action {
            Some(ConsentAction::Dismiss) => {
                self.ui.version_check.consent_dialog = None;
                if request == VersionCheckConsentRequest::Startup {
                    self.preferences.auto_check_updates = false;
                    self.persist_performance_settings();
                }
            }
            Some(ConsentAction::Deny) => {
                self.preferences.github_update_check_allowed = Some(false);
                self.preferences.auto_check_updates = false;
                self.ui.version_check.receiver = None;
                self.ui.version_check.consent_dialog = None;
                self.ui.version_check.dialog = None;
                self.ui.version_check.status = VersionCheckStatus::NotChecked;
                self.persist_performance_settings();
            }
            Some(ConsentAction::Allow) => {
                self.preferences.github_update_check_allowed = Some(true);
                if matches!(
                    request,
                    VersionCheckConsentRequest::Startup
                        | VersionCheckConsentRequest::EnableAutomatic
                ) {
                    self.preferences.auto_check_updates = true;
                }
                self.ui.version_check.consent_dialog = None;
                self.persist_performance_settings();
                if request != VersionCheckConsentRequest::Settings {
                    self.start_version_check(request == VersionCheckConsentRequest::Manual);
                }
            }
            None => {}
        }
    }

    pub(in crate::app) fn show_version_update_dialog(&mut self, ctx: &egui::Context) {
        if self.ui.onboarding_step.is_some() || self.ui.version_check.consent_dialog.is_some() {
            return;
        }
        let Some(release) = self.ui.version_check.dialog.clone() else {
            return;
        };

        enum Action {
            Ignore,
            Remind,
            Update,
        }
        let mut action = None;
        crate::ui::theme::dialog_window(
            egui::Window::new("CalibRaw update available"),
            ctx,
            crate::ui::theme::DIALOG_WIDTH_WIDE,
        )
            .show(ctx, |ui| {
                ui.label(format!(
                    "CalibRaw {} is available. You are using version {}.",
                    release.version,
                    env!("CARGO_PKG_VERSION")
                ));
                if let Some(name) = release
                    .name
                    .as_deref()
                    .filter(|name| *name != release.tag)
                {
                    ui.add_space(crate::ui::theme::SPACE_XS);
                    ui.strong(name);
                }
                ui.add_space(6.0);
                ui.small(
                    "Close ignores this version permanently. Remind me next time shows it again after the next app start.",
                );
                crate::ui::theme::dialog_button_row(ui, |ui| {
                    if crate::ui::theme::secondary_button(ui, "Close").clicked() {
                        action = Some(Action::Ignore);
                    }
                    if crate::ui::theme::secondary_button(ui, "Remind me next time").clicked() {
                        action = Some(Action::Remind);
                    }
                    if crate::ui::theme::primary_action_button(ui, "Update Now").clicked() {
                        action = Some(Action::Update);
                    }
                });
                if action.is_none()
                    && crate::ui::theme::dialog_keyboard_action(
                        ui,
                        crate::ui::theme::DialogKeyboard::CLOSE_ONLY,
                        false,
                    ) == crate::ui::theme::DialogAction::Cancel
                {
                    action = Some(Action::Remind);
                }
            });

        match action {
            Some(Action::Ignore) => {
                self.preferences.ignored_update_version = Some(release.version.to_string());
                self.ui.version_check.status = VersionCheckStatus::Available {
                    version: release.version,
                    ignored: true,
                };
                self.ui.version_check.dialog = None;
                self.persist_performance_settings();
            }
            Some(Action::Remind) => self.ui.version_check.dialog = None,
            Some(Action::Update) => {
                ctx.open_url(egui::OpenUrl::new_tab(&release.url));
                self.ui.version_check.dialog = None;
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_style_tags_are_normalized() {
        assert_eq!(normalized_version("v2.4.1").unwrap().to_string(), "2.4.1");
        assert_eq!(normalized_version("2.4.1").unwrap().to_string(), "2.4.1");
    }

    #[test]
    fn semantic_versions_do_not_use_lexicographic_order() {
        assert!(normalized_version("v2.10.0").unwrap() > normalized_version("2.9.0").unwrap());
    }

    #[test]
    fn invalid_release_tags_are_rejected() {
        assert!(normalized_version("latest").is_err());
    }

    #[test]
    fn github_update_requests_require_explicit_permission() {
        assert!(!github_version_check_permitted(None));
        assert!(!github_version_check_permitted(Some(false)));
        assert!(github_version_check_permitted(Some(true)));
    }

    #[test]
    fn ignored_version_never_reopens_its_popup() {
        assert!(!should_show_update(false, true, true));
        assert!(!should_show_update(true, true, true));
    }

    #[test]
    fn manual_checks_work_when_automatic_checks_are_disabled() {
        assert!(should_show_update(true, false, false));
        assert!(!should_show_update(false, false, false));
    }
}
