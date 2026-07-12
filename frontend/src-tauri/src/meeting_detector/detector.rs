//! Core meeting detection logic
//!
//! Provides process monitoring and meeting detection for Zoom, Teams, and Google Meet.

use crate::meeting_detector::meeting_apps::*;
use log::{debug, info, warn, error};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::path::PathBuf;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::RwLock;

/// Result of inspecting Teams' own windows via System Events.
#[cfg(target_os = "macos")]
struct TeamsCallInfo {
    window_count: usize,
    /// Meeting subject, best-effort — the title segment of whichever window
    /// isn't one of Teams' static left-nav tabs.
    meeting_title: Option<String>,
    /// Email of whichever account is currently active in Teams' account
    /// switcher (the window title reflects the foregrounded account, which
    /// is what matters when multiple accounts are signed into the same
    /// window/switcher rather than separate app instances).
    account_email: Option<String>,
}

#[cfg(target_os = "macos")]
impl TeamsCallInfo {
    fn is_active_call(&self) -> bool {
        self.window_count > 1
    }
}

/// Queries Teams' window titles via System Events (Accessibility/AppleEvents
/// — the packaged app prompts for "Automation" permission for this the
/// first time it runs, same one-time-approval pattern as the mic/screen
/// recording permissions it already requests).
///
/// The Teams process itself runs all day whenever the app is open — it is
/// not, by itself, a signal that a meeting is in progress. Idle, Teams has
/// exactly one window (whatever tab is open — Calendar, Chat, etc). Joining
/// a call always adds a second window for the meeting itself, regardless of
/// UI mode (floating "Meeting compact view" widget, fullscreen, or
/// backgrounded — all three were tested manually via System Events, and the
/// window count was also confirmed live across a real join/leave cycle:
/// count went 1→2 exactly at join, back to 1 exactly at leave).
///
/// Window titles look like "<tab or meeting subject> | <org> | <account
/// email> | Microsoft Teams" (floating-widget mode prefixes one window with
/// "Meeting compact view | "). The meeting window is whichever one's first
/// segment isn't a known static tab name — best-effort, not exhaustive
/// (untested against non-English Teams UI locales).
///
/// Known false-positive: a manually popped-out chat window also raises the
/// count to 2+ without an active call. Accepted for now — same "best
/// effort" spirit as the Google Meet detection below.
#[cfg(target_os = "macos")]
fn query_teams_windows() -> TeamsCallInfo {
    const EMPTY: TeamsCallInfo = TeamsCallInfo {
        window_count: 0,
        meeting_title: None,
        account_email: None,
    };

    let script = r#"tell application "System Events" to tell process "MSTeams"
    set windowNames to name of every window
end tell
set AppleScript's text item delimiters to "|||WINDOW|||"
set outputText to windowNames as text
set AppleScript's text item delimiters to ""
return outputText"#;

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output();

    let raw = match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Ok(out) => {
            // Nonzero exit is how a missing "Automation" permission for
            // System Events shows up (System Settings > Privacy & Security
            // > Automation > Meetily > System Events must be checked) —
            // logged distinctly from "Teams not running"/"idle" so it's
            // diagnosable instead of silently never detecting a meeting.
            warn!(
                "Teams window query failed (check Automation permission for System Events): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return EMPTY;
        }
        Err(e) => {
            warn!("Failed to run osascript for Teams window query: {}", e);
            return EMPTY;
        }
    };

    if raw.is_empty() {
        return EMPTY;
    }

    // Best-effort list of Teams/M365 nav tabs whose name isn't the meeting
    // subject. Not exhaustive — any custom-pinned app tab not in this list
    // could still be mistaken for a title (see known_org below for the
    // structural check that catches the subject-less-call case regardless
    // of this list's completeness).
    const STATIC_TABS: &[&str] = &[
        "Calendar",
        "Chat",
        "Activity",
        "Teams",
        "Calls",
        "Files",
        "Apps",
        "SharePoint",
        "OneDrive",
        "Planner",
        "Forms",
        "Stream",
        "Loop",
        "Whiteboard",
        "Viva Engage",
        "Shifts",
        "Approvals",
        "Bookings",
        "Tasks",
        "To Do",
        "Insights",
        "Copilot",
        "Games",
        "Meeting compact view",
    ];

    let windows: Vec<&str> = raw.split("|||WINDOW|||").collect();
    let window_count = windows.len();
    debug!("Teams windows ({}): {:?}", window_count, windows);

    // Every window's title ends "...<org> | <email> | Microsoft Teams" — the
    // org is always 3rd-from-last, whether or not there's a leading
    // tab/subject segment (a subject-less call's title just collapses to
    // "<org> | <email> | Microsoft Teams", shifting org to the front).
    // Structural, so it works even for tab names not in STATIC_TABS above.
    let known_org: Option<&str> = windows.iter().find_map(|w| {
        let segments: Vec<&str> = w.split('|').map(|s| s.trim()).collect();
        segments.len().checked_sub(3).map(|i| segments[i])
    });

    let mut meeting_title = None;
    let mut account_email = None;

    for w in &windows {
        let segments: Vec<&str> = w.split('|').map(|s| s.trim()).collect();
        // Fewer than 2 segments means the window's title hasn't finished
        // populating yet (still just "Microsoft Teams" or similar) — treat
        // as not-ready rather than mistaking the placeholder for a real
        // title (callers may retry shortly after).
        if segments.len() < 2 {
            continue;
        }

        // Account email is extracted independently of whether this window
        // has a real meeting subject — a subject-less call ("Meet now"
        // with no title set) still has a real signed-in account.
        if account_email.is_none() {
            account_email = segments.iter().find(|s| s.contains('@')).map(|s| s.to_string());
        }

        if meeting_title.is_some() {
            continue;
        }

        let first = segments[0];
        let title_segment = if first == "Meeting compact view" && segments.len() > 2 {
            segments[1]
        } else {
            first
        };

        if STATIC_TABS.contains(&title_segment) {
            continue;
        }
        if Some(title_segment) == known_org {
            // No real subject set for this call — leave meeting_title as
            // None rather than mistaking the org name for the subject.
            continue;
        }

        meeting_title = Some(title_segment.to_string());
    }

    TeamsCallInfo {
        window_count,
        meeting_title,
        account_email,
    }
}

#[cfg(not(target_os = "macos"))]
struct TeamsCallInfo {
    meeting_title: Option<String>,
    account_email: Option<String>,
}

#[cfg(not(target_os = "macos"))]
impl TeamsCallInfo {
    fn is_active_call(&self) -> bool {
        // No equivalent window-inspection signal implemented yet on
        // Windows/Linux — falls back to "Teams process is running" (the
        // old, less precise behavior) by always reporting active.
        true
    }
}

#[cfg(not(target_os = "macos"))]
fn query_teams_windows() -> TeamsCallInfo {
    TeamsCallInfo {
        meeting_title: None,
        account_email: None,
    }
}

/// Represents a detected meeting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedMeeting {
    /// Name of the meeting application (e.g., "Zoom", "Microsoft Teams", "Google Meet")
    pub app_name: String,
    /// Process name that was detected
    pub process_name: String,
    /// Timestamp when the meeting was detected
    pub detected_at: String,
    /// Whether this is an active meeting (vs just the app running)
    pub is_active_meeting: bool,
    /// Meeting subject, when available (currently Teams only, macOS only)
    #[serde(default)]
    pub meeting_title: Option<String>,
    /// Account email associated with the meeting, when available (currently
    /// Teams only, macOS only — reflects whichever account is active in
    /// Teams' account switcher)
    #[serde(default)]
    pub account_email: Option<String>,
}

/// Settings for meeting detection behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingDetectionSettings {
    /// Whether meeting detection is enabled
    pub enabled: bool,
    /// Automatically start recording when a meeting is detected
    pub auto_start_recording: bool,
    /// Automatically stop recording when a meeting ends
    pub auto_stop_recording: bool,
    /// Detect Zoom meetings
    pub detect_zoom: bool,
    /// Detect Microsoft Teams meetings
    pub detect_teams: bool,
    /// Detect Google Meet meetings (requires browser window inspection)
    pub detect_google_meet: bool,
    /// Show notification when a meeting is detected
    pub notify_on_detection: bool,
    /// Polling interval in seconds
    pub poll_interval_secs: u64,
}

impl Default for MeetingDetectionSettings {
    fn default() -> Self {
        Self {
            enabled: false, // Opt-in by default for privacy
            auto_start_recording: false,
            auto_stop_recording: true,
            detect_zoom: true,
            detect_teams: true,
            detect_google_meet: true,
            notify_on_detection: true,
            poll_interval_secs: 5,
        }
    }
}

impl MeetingDetectionSettings {
    /// Get the settings file path
    fn settings_path() -> Option<PathBuf> {
        dirs::data_dir().map(|p| p.join("com.meetily.ai").join("meeting_detection_settings.json"))
    }

    /// Load settings from disk
    pub fn load() -> Self {
        if let Some(path) = Self::settings_path() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => {
                        match serde_json::from_str(&contents) {
                            Ok(settings) => {
                                info!("Loaded meeting detection settings from {:?}", path);
                                return settings;
                            }
                            Err(e) => {
                                error!("Failed to parse meeting detection settings: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to read meeting detection settings: {}", e);
                    }
                }
            }
        }
        Self::default()
    }

    /// Save settings to disk
    pub fn save(&self) -> Result<(), String> {
        if let Some(path) = Self::settings_path() {
            // Ensure parent directory exists
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create settings directory: {}", e))?;
            }

            let contents = serde_json::to_string_pretty(self)
                .map_err(|e| format!("Failed to serialize settings: {}", e))?;

            std::fs::write(&path, contents)
                .map_err(|e| format!("Failed to write settings: {}", e))?;

            info!("Saved meeting detection settings to {:?}", path);
            Ok(())
        } else {
            Err("Could not determine settings path".to_string())
        }
    }
}

/// Status of the meeting detection monitor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingDetectionStatus {
    /// Whether the monitor is currently running
    pub is_monitoring: bool,
    /// Currently detected meeting, if any
    pub current_meeting: Option<DetectedMeeting>,
    /// Current settings
    pub settings: MeetingDetectionSettings,
    /// Whether recording was auto-started by the detector
    pub auto_recording_active: bool,
}

/// Meeting detector that monitors for video conferencing applications
pub struct MeetingDetector {
    system: System,
    settings: Arc<RwLock<MeetingDetectionSettings>>,
    is_monitoring: Arc<AtomicBool>,
    current_meeting: Arc<RwLock<Option<DetectedMeeting>>>,
    auto_recording_active: Arc<AtomicBool>,
}

impl MeetingDetector {
    /// Create a new meeting detector with settings loaded from disk
    pub fn new() -> Self {
        // Load persisted settings or use defaults
        let loaded_settings = MeetingDetectionSettings::load();
        info!("MeetingDetector initialized with settings: enabled={}, auto_start={}",
              loaded_settings.enabled, loaded_settings.auto_start_recording);

        Self {
            system: System::new_with_specifics(
                RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
            ),
            settings: Arc::new(RwLock::new(loaded_settings)),
            is_monitoring: Arc::new(AtomicBool::new(false)),
            current_meeting: Arc::new(RwLock::new(None)),
            auto_recording_active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get current settings
    pub async fn get_settings(&self) -> MeetingDetectionSettings {
        self.settings.read().await.clone()
    }

    /// Update settings and persist to disk
    pub async fn set_settings(&self, settings: MeetingDetectionSettings) {
        // Save to disk first
        if let Err(e) = settings.save() {
            error!("Failed to save meeting detection settings: {}", e);
        }

        let mut current = self.settings.write().await;
        *current = settings;
    }

    /// Check if monitoring is active
    pub fn is_monitoring(&self) -> bool {
        self.is_monitoring.load(Ordering::SeqCst)
    }

    /// Get current detection status
    pub async fn get_status(&self) -> MeetingDetectionStatus {
        MeetingDetectionStatus {
            is_monitoring: self.is_monitoring(),
            current_meeting: self.current_meeting.read().await.clone(),
            settings: self.get_settings().await,
            auto_recording_active: self.auto_recording_active.load(Ordering::SeqCst),
        }
    }

    /// Detect if any meeting application is running
    pub fn detect_meeting(&mut self, settings: &MeetingDetectionSettings) -> Option<DetectedMeeting> {
        self.system.refresh_processes(ProcessesToUpdate::All, true);

        for (_pid, process) in self.system.processes() {
            let name = process.name().to_string_lossy().to_lowercase();

            // Check for Zoom - only detect ACTIVE meetings, not just the app being open
            if settings.detect_zoom {
                // CptHost is the process that runs during an active Zoom meeting on macOS
                // This is more reliable than just detecting zoom.us which runs when app is open
                if name.contains("cpthost") {
                    info!("Detected active Zoom meeting via CptHost process");
                    return Some(DetectedMeeting {
                        app_name: "Zoom".to_string(),
                        process_name: process.name().to_string_lossy().to_string(),
                        detected_at: chrono::Local::now().to_rfc3339(),
                        is_active_meeting: true,
                        meeting_title: None,
                        account_email: None,
                    });
                }
            }

            // Check for Microsoft Teams — process presence alone isn't enough
            // (Teams stays open all day); only report a meeting if there's
            // also an active-call window (see query_teams_windows).
            if settings.detect_teams {
                let teams_running = TEAMS_PROCESSES
                    .iter()
                    .any(|p| name.contains(&p.to_lowercase()));
                if teams_running {
                    let call_info = query_teams_windows();
                    if call_info.is_active_call() {
                        return Some(DetectedMeeting {
                            app_name: "Microsoft Teams".to_string(),
                            process_name: process.name().to_string_lossy().to_string(),
                            detected_at: chrono::Local::now().to_rfc3339(),
                            is_active_meeting: true,
                            meeting_title: call_info.meeting_title,
                            account_email: call_info.account_email,
                        });
                    }
                }
            }

            // Check for Google Meet (browser-based)
            // This requires platform-specific window title detection
            if settings.detect_google_meet {
                if let Some(meeting) = self.detect_google_meet_in_browser(&name, process) {
                    return Some(meeting);
                }
            }
        }

        None
    }

    /// Detect Google Meet running in a browser
    /// This is a simplified check - full implementation requires window title inspection
    #[cfg(target_os = "macos")]
    fn detect_google_meet_in_browser(
        &self,
        process_name: &str,
        _process: &sysinfo::Process,
    ) -> Option<DetectedMeeting> {
        // On macOS, we can use accessibility APIs to check window titles
        // For now, we'll use a simplified approach that checks for browser processes
        // A full implementation would use the Accessibility framework

        for browser in BROWSER_PROCESSES {
            if process_name.contains(&browser.to_lowercase()) {
                // TODO: Implement window title checking via Accessibility API
                // For now, we can't reliably detect Google Meet without window inspection
                // This would require checking if any window title contains "meet.google.com"
                debug!(
                    "Browser detected: {} - Google Meet detection requires window title inspection",
                    browser
                );
            }
        }

        None
    }

    #[cfg(not(target_os = "macos"))]
    fn detect_google_meet_in_browser(
        &self,
        _process_name: &str,
        _process: &sysinfo::Process,
    ) -> Option<DetectedMeeting> {
        // On Windows/Linux, window title detection requires platform-specific APIs
        // Windows: EnumWindows + GetWindowText
        // Linux: X11/Wayland APIs
        None
    }

    /// Start the background monitoring task
    pub async fn start_monitoring<R: Runtime>(&self, app: AppHandle<R>) {
        if self.is_monitoring.load(Ordering::SeqCst) {
            warn!("Meeting detection is already running");
            return;
        }

        self.is_monitoring.store(true, Ordering::SeqCst);
        info!("Starting meeting detection monitor");

        let is_monitoring = self.is_monitoring.clone();
        let settings = self.settings.clone();
        let current_meeting = self.current_meeting.clone();
        let auto_recording_active = self.auto_recording_active.clone();

        tokio::spawn(async move {
            let mut system = System::new_with_specifics(
                RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
            );
            let mut was_in_meeting = false;

            while is_monitoring.load(Ordering::SeqCst) {
                let current_settings = settings.read().await.clone();

                if !current_settings.enabled {
                    tokio::time::sleep(Duration::from_secs(current_settings.poll_interval_secs))
                        .await;
                    continue;
                }

                system.refresh_processes(ProcessesToUpdate::All, true);

                // Detect meeting using inline logic (can't call &mut self in spawned task)
                let meeting = detect_meeting_from_system(&system, &current_settings);

                match (was_in_meeting, meeting.is_some()) {
                    (false, true) => {
                        // Meeting started
                        let meeting_info = meeting.unwrap();
                        info!(
                            "Meeting detected: {} ({})",
                            meeting_info.app_name, meeting_info.process_name
                        );

                        // Store current meeting
                        {
                            let mut current = current_meeting.write().await;
                            *current = Some(meeting_info.clone());
                        }

                        // Emit event to frontend
                        let _ = app.emit("meeting-detected", &meeting_info);

                        // Show notification if enabled
                        if current_settings.notify_on_detection {
                            let _ = app.emit(
                                "meeting-detection-notification",
                                serde_json::json!({
                                    "title": format!("{} Meeting Detected", meeting_info.app_name),
                                    "body": "Click to start recording"
                                }),
                            );
                        }

                        // Auto-start recording if enabled
                        if current_settings.auto_start_recording {
                            // Teams' meeting window is created before its
                            // title finishes populating with the real
                            // subject ("<title> | <org> | <email> |
                            // Microsoft Teams") — a query right at
                            // detection can still catch the bare app-name
                            // placeholder. Retry briefly so the recording
                            // gets a real title instead of the generic
                            // fallback.
                            #[cfg(target_os = "macos")]
                            let (retried_title, retried_account) =
                                if meeting_info.app_name == "Microsoft Teams"
                                    && meeting_info.meeting_title.is_none()
                                {
                                    let mut title = None;
                                    let mut account = None;
                                    for _ in 0..4 {
                                        tokio::time::sleep(Duration::from_millis(750)).await;
                                        let info = query_teams_windows();
                                        // Stop as soon as the window is
                                        // fully loaded (account present),
                                        // even if there's genuinely no
                                        // meeting subject to find — no
                                        // point retrying a subject-less
                                        // "Meet now" call 4 times.
                                        if info.meeting_title.is_some() || info.account_email.is_some() {
                                            title = info.meeting_title;
                                            account = info.account_email;
                                            break;
                                        }
                                    }
                                    (title, account)
                                } else {
                                    (
                                        meeting_info.meeting_title.clone(),
                                        meeting_info.account_email.clone(),
                                    )
                                };
                            #[cfg(not(target_os = "macos"))]
                            let (retried_title, retried_account) = (
                                meeting_info.meeting_title.clone(),
                                meeting_info.account_email.clone(),
                            );

                            let meeting_name = match (&retried_title, &retried_account) {
                                (Some(title), Some(account)) => {
                                    format!("{} ({})", title, account)
                                }
                                (Some(title), None) => title.clone(),
                                (None, Some(account)) => {
                                    format!("{} Meeting ({})", meeting_info.app_name, account)
                                }
                                (None, None) => format!("{} Meeting", meeting_info.app_name),
                            };
                            info!("Auto-starting recording for: {}", meeting_name);

                            // Emit event for frontend to handle recording start
                            let _ = app.emit(
                                "auto-start-recording",
                                serde_json::json!({
                                    "meeting_name": meeting_name,
                                    "app_name": meeting_info.app_name
                                }),
                            );

                            auto_recording_active.store(true, Ordering::SeqCst);
                        }

                        was_in_meeting = true;
                    }
                    (true, false) => {
                        // Meeting ended
                        info!("Meeting ended");

                        // Clear current meeting
                        {
                            let mut current = current_meeting.write().await;
                            *current = None;
                        }

                        // Emit event to frontend
                        let _ = app.emit("meeting-ended", ());

                        // Auto-stop recording if enabled and we auto-started.
                        // Calls the same Rust stop_recording command the tray
                        // menu uses directly (crate::audio::recording_commands),
                        // then emits recording-stop-complete — the event
                        // RecordingPostProcessingProvider already listens for
                        // from every other stop source (tray, shortcut, main
                        // UI). A frontend-only "auto-stop-recording" event
                        // (the previous approach) never actually stopped the
                        // backend recording, only ran post-processing on
                        // whatever was already there.
                        if current_settings.auto_stop_recording
                            && auto_recording_active.load(Ordering::SeqCst)
                        {
                            info!("Auto-stopping recording");
                            auto_recording_active.store(false, Ordering::SeqCst);

                            if let Ok(data_dir) = app.path().app_data_dir() {
                                let timestamp = chrono::Local::now()
                                    .format("%Y-%m-%dT%H-%M-%S")
                                    .to_string();
                                let save_path =
                                    data_dir.join(format!("recording-{}.wav", timestamp));

                                let stop_result = crate::audio::recording_commands::stop_recording(
                                    app.clone(),
                                    crate::audio::recording_commands::RecordingArgs {
                                        save_path: save_path.to_string_lossy().to_string(),
                                    },
                                )
                                .await;

                                match stop_result {
                                    Ok(_) => {
                                        info!("Auto-stop: recording stopped successfully");
                                        let _ = app.emit("recording-stop-complete", true);
                                    }
                                    Err(e) => {
                                        error!("Auto-stop: failed to stop recording: {}", e);
                                    }
                                }
                            } else {
                                error!("Auto-stop: failed to resolve app data dir");
                            }
                        }

                        was_in_meeting = false;
                    }
                    _ => {} // No state change
                }

                tokio::time::sleep(Duration::from_secs(current_settings.poll_interval_secs)).await;
            }

            info!("Meeting detection monitor stopped");
        });
    }

    /// Stop the background monitoring task
    pub fn stop_monitoring(&self) {
        info!("Stopping meeting detection monitor");
        self.is_monitoring.store(false, Ordering::SeqCst);
    }
}

impl Default for MeetingDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper function to detect meetings from a System instance
/// Used in the spawned monitoring task
fn detect_meeting_from_system(
    system: &System,
    settings: &MeetingDetectionSettings,
) -> Option<DetectedMeeting> {
    for (_pid, process) in system.processes() {
        let name = process.name().to_string_lossy().to_lowercase();

        // Check for Zoom - only detect ACTIVE meetings via CptHost process
        if settings.detect_zoom {
            // CptHost is the process that runs during an active Zoom meeting on macOS
            if name.contains("cpthost") {
                return Some(DetectedMeeting {
                    app_name: "Zoom".to_string(),
                    process_name: process.name().to_string_lossy().to_string(),
                    detected_at: chrono::Local::now().to_rfc3339(),
                    is_active_meeting: true,
                    meeting_title: None,
                    account_email: None,
                });
            }
        }

        // Check for Microsoft Teams — same active-call window check as above.
        if settings.detect_teams {
            let teams_running = TEAMS_PROCESSES
                .iter()
                .any(|p| name.contains(&p.to_lowercase()));
            if teams_running {
                let call_info = query_teams_windows();
                if call_info.is_active_call() {
                    return Some(DetectedMeeting {
                        app_name: "Microsoft Teams".to_string(),
                        process_name: process.name().to_string_lossy().to_string(),
                        detected_at: chrono::Local::now().to_rfc3339(),
                        is_active_meeting: true,
                        meeting_title: call_info.meeting_title,
                        account_email: call_info.account_email,
                    });
                }
            }
        }
    }

    None
}
