// Radio indices couple OPEN_BEHAVIORS and OPEN_BEHAVIOR_LABELS; candidate queries settle off-thread.

// The Open With subtitle prefix, ending at an opening curly quote; joined only by
// [`OpenWithState::chooser_subtitle`].
pub(crate) const CHOOSE_APP_PREFIX: &str = "Choose an application to open \u{201c}";

// The closing curly quote after the file name in the subtitle.
pub(crate) const CHOOSE_APP_SUFFIX: &str = "\u{201d}";

// A single application that can open the target file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OpenCandidate {
    // Freedesktop desktop-file ID used to match the default application.
    pub(crate) desktop_id: String,
    // Unadorned application name, used by the auto-detect hint and sorting.
    pub(crate) name: String,
    // Row label; the default candidate carries a textual ` (Default)` suffix.
    pub(crate) display_name: String,
    // The structured `gio launch <desktop-file>`; `gio` interprets the Exec field
    // codes.
    pub(crate) command: crate::launch::LaunchSpec,
    // Whether the desktop default query named this desktop-file ID.
    pub(crate) is_default: bool,
    // A text editor offered for a file it does not advertise.
    pub(crate) as_text: bool,
    // The application's icon, ready to draw.
    pub(crate) icon: Option<CandidateIcon>,
}

// An application icon, decoded once when the candidate is built.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CandidateIcon {
    Svg(iced::widget::svg::Handle),
    Raster(iced::widget::image::Handle),
}

impl Eq for CandidateIcon {}

impl CandidateIcon {
    // Load the icon at `path`: SVG lazily, a raster through the bounded
    // decoder.
    fn load(path: &std::path::Path) -> Option<Self> {
        if path.extension().is_some_and(|ext| ext == "svg") {
            return Some(Self::Svg(iced::widget::svg::Handle::from_path(path)));
        }
        let bytes = std::fs::read(path).ok()?;
        let limits = enzui_core::decode_limits::DecodeLimits::DEFAULT;
        enzui_resources::decode_static(&bytes, &limits, || false)
            .ok()
            .map(|decoded| Self::Raster(decoded.handle))
    }
}

impl OpenCandidate {
    fn from_desktop(app: crate::mime_query::DesktopApplication) -> Self {
        let command = app.launch_spec();
        let icon = app.icon.as_deref().and_then(CandidateIcon::load);
        let name = app.name;
        Self {
            desktop_id: app.desktop_id,
            display_name: name.clone(),
            name,
            command,
            is_default: false,
            as_text: app.as_text,
            icon,
        }
    }

    fn mark_default(&mut self) {
        self.is_default = true;
        self.display_name =
            crate::i18n::format("files-default-fd8fc33d", &[("arg0", (self.name).clone())]);
    }

    // The rendered row label, including the textual default marker.
    pub(crate) fn rendered_name(&self) -> &str {
        &self.display_name
    }
}

// How opening this kind of file should behave.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpenBehavior {
    // Open with the single chosen application every time.
    Always,
    // Ask which application each time.
    Ask,
}

// The "Open Behavior" radio labels in display order, parallel to
// [`OPEN_BEHAVIORS`].
pub(crate) const OPEN_BEHAVIOR_LABELS: [&str; 2] = ["Always this app", "Ask each time"];
pub(crate) const OPEN_BEHAVIORS: [OpenBehavior; 2] = [OpenBehavior::Always, OpenBehavior::Ask];

// The index of `behavior` in [`OPEN_BEHAVIORS`].
pub(crate) fn behavior_index(behavior: OpenBehavior) -> usize {
    OPEN_BEHAVIORS
        .iter()
        .position(|b| *b == behavior)
        .unwrap_or(0)
}

// The [`OpenBehavior`] at `index`, or [`OpenBehavior::Always`] when out of range.
pub(crate) fn behavior_from_index(index: usize) -> OpenBehavior {
    OPEN_BEHAVIORS
        .get(index)
        .copied()
        .unwrap_or(OpenBehavior::Always)
}

// State of the installed-application query shown inside the dialog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CandidateState {
    // The off-thread query has not returned yet.
    Loading,
    // At least one installed handler is available.
    Ready,
    // The application database was read, but no handler advertises this MIME.
    Empty,
    // The application database itself could not be read.
    Unavailable(String),
}

// The Open With / How to Open dialog state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OpenWithState {
    // The target file name, for the title and auto-detect hint.
    pub(crate) target_name: String,
    // The target's full path, set by `Msg::ShowOpenWith`; `None` for bare-name
    // construction.
    pub(crate) target_path: Option<std::path::PathBuf>,
    // The mime type of the target, used to pick the auto-detected app.
    pub(crate) mime: String,
    // Every application offered, default first.
    pub(crate) candidates: Vec<OpenCandidate>,
    // The selected index into `candidates`.
    pub(crate) selected: Option<usize>,
    // The chosen open behavior.
    pub(crate) behavior: OpenBehavior,
    // The free-text "Or type a command" field.
    pub(crate) custom_command: String,
    // Whether handlers are loading, ready, honestly empty, or unavailable.
    pub(crate) candidate_state: CandidateState,
    // A non-fatal default-query notice; handlers still render without a successful default query
    // or a registered default.
    pub(crate) default_notice: Option<String>,
}

impl OpenWithState {
    // Builds a loading dialog for a target of `mime`; the reducer later calls
    // [`Self::apply_query`].
    pub(crate) fn for_target(target_name: impl Into<String>, mime: impl Into<String>) -> Self {
        Self {
            target_name: target_name.into(),
            target_path: None,
            mime: mime.into(),
            candidates: Vec::new(),
            selected: None,
            behavior: OpenBehavior::Ask,
            custom_command: String::new(),
            candidate_state: CandidateState::Loading,
            default_notice: None,
        }
    }

    // Installs the injected query's handler and default results.
    //
    // A handler-enumeration failure is a visible unavailable state. A default-query
    // failure only adds a notice. A known default is marked and sorted first;
    // otherwise handlers stay alphabetical.
    pub(crate) fn apply_query(&mut self, outcome: crate::mime_query::MimeQueryOutcome) {
        let applications = match outcome.applications {
            Ok(applications) => applications,
            Err(reason) => {
                self.candidates.clear();
                self.selected = None;
                self.default_notice = None;
                self.candidate_state = CandidateState::Unavailable(crate::i18n::format(
                    "files-could-not-read-installed-applications-reason-2950301d",
                    &[("reason", (reason))],
                ));
                return;
            }
        };

        self.candidates = applications
            .into_iter()
            .map(OpenCandidate::from_desktop)
            .collect();
        self.default_notice = match outcome.default {
            Ok(Some(default_id)) => {
                if let Some(candidate) = self
                    .candidates
                    .iter_mut()
                    .find(|candidate| candidate.desktop_id == default_id)
                {
                    candidate.mark_default();
                    None
                } else {
                    Some(crate::i18n::format(
                        "files-the-default-application-default-id-is-8acfad2d",
                        &[("default_id", (default_id))],
                    ))
                }
            }
            Ok(None) => Some(crate::i18n::format(
                "files-no-default-application-is-registered-for-78c0dc73",
                &[("arg0", (self.mime).clone())],
            )),
            Err(reason) => Some(crate::i18n::format(
                "files-default-application-unavailable-reason-d3e3a6ab",
                &[("reason", (reason))],
            )),
        };

        self.candidates.sort_by(|a, b| {
            b.is_default
                .cmp(&a.is_default)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.desktop_id.cmp(&b.desktop_id))
        });
        self.selected = if self.candidates.is_empty() {
            None
        } else {
            Some(0)
        };
        self.candidate_state = if self.candidates.is_empty() {
            CandidateState::Empty
        } else {
            CandidateState::Ready
        };
    }

    // The Open With subtitle, such as `Choose an application to open “notes.txt”`.
    pub(crate) fn chooser_subtitle(&self) -> String {
        format!(
            "{}{}{}",
            CHOOSE_APP_PREFIX, self.target_name, CHOOSE_APP_SUFFIX
        )
    }

    // The launch "Open" would run: the typed command when non-empty, else the
    // selected candidate. `None` when neither exists.
    //
    // Only the typed field is split on whitespace ([`LaunchSpec::parse_custom`]);
    // installed handlers launch by desktop-file path.
    //
    // [`LaunchSpec::parse_custom`]: crate::launch::LaunchSpec::parse_custom
    pub(crate) fn effective_command(&self) -> Option<crate::launch::LaunchSpec> {
        if let Some(custom) = crate::launch::LaunchSpec::parse_custom(&self.custom_command) {
            return Some(custom);
        }
        self.selected
            .and_then(|i| self.candidates.get(i))
            .map(|c| c.command.clone())
    }
}
