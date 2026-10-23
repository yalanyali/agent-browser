//! Ardia connection mode (`--ardia`).
//!
//! Ardia is a Chromium browser that people use as their main browser. It
//! exposes CDP to local tools through a token-gated endpoint described by
//! `ArdiaControl.json`, and enforces a CDP method allowlist inside the browser
//! for every client that uses the token. Everything specific to Ardia lives in
//! this module so upstream merges stay cheap:
//!
//! - [`resolve_endpoint`] reads the control file with the same checks as the
//!   `ardia` CLI (`apps/cli/ardia_cli/endpoint.py` in the Ardia repo).
//! - [`FocusPolicy`] decides whether agent-browser may move the user's focus.
//! - [`ALLOWED_METHODS`] and [`unsupported_action`] are the capability list.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const CONTROL_FILE: &str = "ArdiaControl.json";

const NOT_REACHABLE: &str =
    "Ardia isn't running, or \"Let the ardia command control Ardia\" is off in Settings";
const MAX_CONTROL_FILE: u64 = 64 * 1024;

/// Whether agent-browser may change which tab or window the user sees.
///
/// Upstream behavior steals focus freely: it activates a tab whose renderer
/// does not answer, brings switched-to tabs to the front, and opens new tabs
/// in the foreground. That is fine for a browser the agent owns, but not for
/// the browser a person is working in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusPolicy {
    pub may_steal_focus: bool,
}

impl FocusPolicy {
    pub const UPSTREAM: Self = Self {
        may_steal_focus: true,
    };

    pub fn ardia(front: bool) -> Self {
        Self {
            may_steal_focus: front,
        }
    }
}

/// Error for a tab whose renderer does not answer when activating it is not
/// allowed. Ardia parks background tabs; activating one would focus it.
pub fn parked_tab_error() -> String {
    "Ardia has parked this tab. Open it in Ardia first, or pass --front to let \
     agent-browser activate it (this focuses the tab)"
        .to_string()
}

/// Location of Ardia's data directory: `ARDIA_DATA_DIR` when set (a separate
/// test instance), otherwise the installed app's directory.
pub fn data_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("ARDIA_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join("Library/Application Support/com.ardia.browser")
}

pub fn control_file_path() -> PathBuf {
    data_root().join("Chromium").join(CONTROL_FILE)
}

#[derive(Deserialize)]
struct Control {
    port: i64,
    browser: String,
    token: String,
    pid: i64,
}

/// Read the control file and return the browser WebSocket URL. The file is
/// re-read on every connect, so a restarted Ardia (new port, token and pid)
/// is picked up without any user action.
pub fn resolve_endpoint() -> Result<String, String> {
    resolve_endpoint_from(&control_file_path())
}

fn resolve_endpoint_from(path: &Path) -> Result<String, String> {
    let name = CONTROL_FILE;
    let raw = read_private_file(path)?;
    let control: Control = serde_json::from_slice(&raw)
        .map_err(|_| format!("{} ({} isn't valid JSON)", NOT_REACHABLE, name))?;
    if !(1..65536).contains(&control.port) {
        return Err(format!("{} ({} has no valid port)", NOT_REACHABLE, name));
    }
    if !valid_browser_path(&control.browser) {
        return Err(format!(
            "{} ({} has no valid browser path)",
            NOT_REACHABLE, name
        ));
    }
    if !valid_token(&control.token) {
        return Err(format!("{} ({} has no valid token)", NOT_REACHABLE, name));
    }
    if control.pid <= 1 || !pid_alive(control.pid) {
        return Err(NOT_REACHABLE.to_string());
    }
    Ok(format!(
        "ws://127.0.0.1:{}{}?token={}",
        control.port,
        control.browser,
        urlencoding::encode(&control.token)
    ))
}

#[cfg(unix)]
fn read_private_file(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let name = CONTROL_FILE;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => NOT_REACHABLE.to_string(),
            _ if e.raw_os_error() == Some(libc::ELOOP) => {
                format!("{} ({} is a symbolic link)", NOT_REACHABLE, name)
            }
            _ => format!("{} ({} {})", NOT_REACHABLE, name, e),
        })?;
    let meta = file
        .metadata()
        .map_err(|e| format!("{} ({} {})", NOT_REACHABLE, name, e))?;
    if !meta.is_file() {
        return Err(format!("{} ({} isn't a regular file)", NOT_REACHABLE, name));
    }
    if meta.uid() != unsafe { libc::getuid() } {
        return Err(format!(
            "{} ({} belongs to another user)",
            NOT_REACHABLE, name
        ));
    }
    if meta.permissions().mode() & 0o7777 != 0o600 {
        return Err(format!("{} ({} must be mode 0600)", NOT_REACHABLE, name));
    }
    let mut raw = Vec::new();
    file.take(MAX_CONTROL_FILE + 1)
        .read_to_end(&mut raw)
        .map_err(|e| format!("{} ({} {})", NOT_REACHABLE, name, e))?;
    Ok(raw)
}

#[cfg(not(unix))]
fn read_private_file(_path: &Path) -> Result<Vec<u8>, String> {
    Err("--ardia is only supported on macOS".to_string())
}

#[cfg(unix)]
fn pid_alive(pid: i64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: i64) -> bool {
    false
}

/// `^/devtools/browser/[0-9A-Za-z-]{1,128}$`
fn valid_browser_path(path: &str) -> bool {
    path.strip_prefix("/devtools/browser/").is_some_and(|id| {
        (1..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// `^[\x21-\x7e]{16,1024}$`
fn valid_token(token: &str) -> bool {
    (16..=1024).contains(&token.len()) && token.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// CDP methods Ardia accepts from token clients. Copied from `AllowedMethods()`
/// in `engine/chromium/src/chrome/browser/ardia/ac_cli_token_policy.cc`
/// (Ardia b833d76). Anything else is refused with -32601, so the CDP client
/// fails it locally instead of sending it. Update this list when Ardia's
/// allowlist changes.
pub const ALLOWED_METHODS: &[&str] = &[
    "Accessibility.disable",
    "Accessibility.enable",
    "Accessibility.getChildAXNodes",
    "Accessibility.getFullAXTree",
    "Accessibility.getPartialAXTree",
    "Accessibility.queryAXTree",
    "Browser.getVersion",
    "DOM.describeNode",
    "DOM.disable",
    "DOM.enable",
    "DOM.focus",
    "DOM.getAttributes",
    "DOM.getBoxModel",
    "DOM.getContentQuads",
    "DOM.getDocument",
    "DOM.getFrameOwner",
    "DOM.getNodeForLocation",
    "DOM.getOuterHTML",
    "DOM.querySelector",
    "DOM.querySelectorAll",
    "DOM.requestChildNodes",
    "DOM.requestNode",
    "DOM.resolveNode",
    "DOM.scrollIntoViewIfNeeded",
    "Input.dispatchKeyEvent",
    "Input.dispatchMouseEvent",
    "Input.dispatchTouchEvent",
    "Input.insertText",
    "Network.disable",
    "Network.enable",
    "Network.getCertificate",
    "Network.getRequestPostData",
    "Network.getResponseBody",
    "Network.getSecurityIsolationStatus",
    "Network.searchInResponseBody",
    "Network.streamResourceContent",
    "Page.addScriptToEvaluateOnNewDocument",
    "Page.bringToFront",
    "Page.captureScreenshot",
    "Page.close",
    "Page.createIsolatedWorld",
    "Page.disable",
    "Page.enable",
    "Page.getFrameTree",
    "Page.getLayoutMetrics",
    "Page.getNavigationHistory",
    "Page.handleJavaScriptDialog",
    "Page.navigate",
    "Page.printToPDF",
    "Page.reload",
    "Page.removeScriptToEvaluateOnNewDocument",
    "Page.setLifecycleEventsEnabled",
    "Page.stopLoading",
    "Runtime.awaitPromise",
    "Runtime.callFunctionOn",
    "Runtime.disable",
    "Runtime.enable",
    "Runtime.evaluate",
    "Runtime.getProperties",
    "Runtime.releaseObject",
    "Runtime.releaseObjectGroup",
    "Runtime.runIfWaitingForDebugger",
    "Target.activateTarget",
    "Target.attachToTarget",
    "Target.closeTarget",
    "Target.createTarget",
    "Target.detachFromTarget",
    "Target.getTargetInfo",
    "Target.getTargets",
    "Target.setAutoAttach",
    "Target.setDiscoverTargets",
];

pub fn method_allowed(method: &str) -> bool {
    ALLOWED_METHODS.contains(&method)
}

/// CDP client guard for Ardia connections.
pub fn check_method(method: &str) -> Result<(), String> {
    if method_allowed(method) {
        Ok(())
    } else {
        Err(format!(
            "{} is not available on Ardia (Ardia's CDP allowlist refuses it)",
            method
        ))
    }
}

/// Daemon actions that cannot work on Ardia because the CDP methods they
/// depend on are refused, with the reason shown to the user. Rejecting them
/// up front avoids half-applied commands. Derived from the handlers' CDP
/// calls (`.ardia-dev/refused_actions.py` in the fork's working tree).
const UNSUPPORTED_ACTIONS: &[(&str, &str)] = &[
    ("cookies_get", "cookie methods"),
    ("cookies_set", "cookie methods"),
    ("cookies_clear", "cookie methods"),
    ("state_save", "cookie methods"),
    ("state_load", "cookie methods"),
    ("setcontent", "Page.setDocumentContent"),
    ("headers", "Network.setExtraHTTPHeaders"),
    ("credentials", "Network.setExtraHTTPHeaders"),
    ("offline", "Network.emulateNetworkConditions"),
    ("route", "Fetch.*"),
    ("unroute", "Fetch.*"),
    ("trace_start", "Tracing.*"),
    ("trace_stop", "Tracing.*"),
    ("profiler_start", "Tracing.*"),
    ("profiler_stop", "Tracing.*"),
    ("viewport", "Emulation.*"),
    ("device", "Emulation.*"),
    ("useragent", "Emulation.*"),
    ("user_agent", "Emulation.*"),
    ("set_media", "Emulation.*"),
    ("emulatemedia", "Emulation.*"),
    ("timezone", "Emulation.*"),
    ("locale", "Emulation.*"),
    ("geolocation", "Emulation.*"),
    ("permissions", "Browser.grantPermissions"),
    ("download", "Browser.setDownloadBehavior"),
    ("waitfordownload", "Browser.setDownloadBehavior"),
    ("upload", "DOM.setFileInputFiles"),
    ("expose", "Runtime.addBinding"),
    ("window_new", "browser contexts"),
    ("screencast_start", "Page.startScreencast"),
    ("screencast_stop", "Page.startScreencast"),
    ("stream_enable", "Page.startScreencast"),
    ("video_start", "Page.startScreencast"),
    ("video_stop", "Page.startScreencast"),
    ("recording_start", "Page.startScreencast"),
    ("recording_restart", "Page.startScreencast"),
    ("recording_stop", "Page.startScreencast"),
    ("webmcp_list", "WebMCP.*"),
    ("webmcp_invoke", "WebMCP.*"),
    ("webmcp_result", "WebMCP.*"),
    ("webmcp_cancel", "WebMCP.*"),
];

/// The "not available on Ardia" error for an action, if it is unsupported.
pub fn unsupported_action(action: &str) -> Option<String> {
    UNSUPPORTED_ACTIONS
        .iter()
        .find(|(name, _)| *name == action)
        .map(|(name, needs)| {
            format!(
                "'{}' is not available on Ardia (it needs {}, which Ardia's CDP allowlist refuses)",
                name, needs
            )
        })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_control(dir: &Path, body: &str, mode: u32) -> PathBuf {
        let path = dir.join(CONTROL_FILE);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn control_json(pid: u32) -> String {
        format!(
            r#"{{"port":9333,"browser":"/devtools/browser/abc-123","token":"tok/en+0123456789abcdef","pid":{},"generation":4}}"#,
            pid
        )
    }

    #[test]
    fn builds_endpoint_with_encoded_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_control(dir.path(), &control_json(std::process::id()), 0o600);
        assert_eq!(
            resolve_endpoint_from(&path).unwrap(),
            "ws://127.0.0.1:9333/devtools/browser/abc-123?token=tok%2Fen%2B0123456789abcdef"
        );
    }

    #[test]
    fn rejects_loose_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_control(dir.path(), &control_json(std::process::id()), 0o644);
        assert!(resolve_endpoint_from(&path)
            .unwrap_err()
            .contains("mode 0600"));
    }

    #[test]
    fn rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = write_control(dir.path(), &control_json(std::process::id()), 0o600);
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(resolve_endpoint_from(&link)
            .unwrap_err()
            .contains("symbolic link"));
    }

    #[test]
    fn rejects_dead_pid() {
        let dir = tempfile::tempdir().unwrap();
        // pid_max on macOS is 99998; this pid cannot exist.
        let path = write_control(dir.path(), &control_json(999_999), 0o600);
        assert_eq!(resolve_endpoint_from(&path).unwrap_err(), NOT_REACHABLE);
    }

    #[test]
    fn missing_file_is_not_reachable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_endpoint_from(&dir.path().join(CONTROL_FILE)).unwrap_err(),
            NOT_REACHABLE
        );
    }

    #[test]
    fn rejects_bad_fields() {
        let dir = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        for body in [
            format!(
                r#"{{"port":0,"browser":"/devtools/browser/a","token":"0123456789abcdef","pid":{pid}}}"#
            ),
            format!(
                r#"{{"port":9,"browser":"/devtools/page/a","token":"0123456789abcdef","pid":{pid}}}"#
            ),
            format!(r#"{{"port":9,"browser":"/devtools/browser/a","token":"short","pid":{pid}}}"#),
            "[]".to_string(),
        ] {
            let path = write_control(dir.path(), &body, 0o600);
            assert!(resolve_endpoint_from(&path).is_err(), "{body}");
        }
    }

    #[test]
    fn allowlist_matches_known_refusals() {
        for refused in [
            "Emulation.setDeviceMetricsOverride",
            "Fetch.enable",
            "Storage.getCookies",
            "Runtime.addBinding",
            "Browser.setDownloadBehavior",
            "Target.getBrowserContexts",
            "DOM.setFileInputFiles",
            "WebMCP.enable",
        ] {
            assert!(!method_allowed(refused), "{refused}");
        }
        for allowed in ["Target.createTarget", "Page.navigate", "Runtime.evaluate"] {
            assert!(method_allowed(allowed), "{allowed}");
        }
    }

    #[test]
    fn unsupported_actions_name_the_reason() {
        let err = unsupported_action("viewport").unwrap();
        assert!(err.contains("not available on Ardia"), "{err}");
        assert!(unsupported_action("snapshot").is_none());
    }

    #[test]
    fn focus_policy() {
        assert!(FocusPolicy::UPSTREAM.may_steal_focus);
        assert!(!FocusPolicy::ardia(false).may_steal_focus);
        assert!(FocusPolicy::ardia(true).may_steal_focus);
    }
}
