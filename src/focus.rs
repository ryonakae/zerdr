use std::fs::OpenOptions;
use std::io::Write;

use crate::state::RouteFocus;

const ZED_BUNDLE_IDENTIFIERS: [&str; 4] = [
    "dev.zed.Zed",
    "dev.zed.Zed-Preview",
    "dev.zed.Zed-Nightly",
    "dev.zed.Zed-Dev",
];

pub fn with_external_focus<R>(focus: RouteFocus, operation: impl FnOnce() -> R) -> R {
    if focus == RouteFocus::Zed {
        return operation();
    }
    if test_backend_enabled() {
        return with_test_restoration(operation);
    }
    #[cfg(target_os = "macos")]
    {
        macos::with_restoration(operation)
    }
    #[cfg(not(target_os = "macos"))]
    {
        operation()
    }
}

fn is_zed_bundle_identifier(identifier: &str) -> bool {
    ZED_BUNDLE_IDENTIFIERS.contains(&identifier)
}

fn test_backend_enabled() -> bool {
    std::env::var_os("ZERDR_TEST_ROOT").is_some()
        && std::env::var("ZERDR_TEST_FOCUS_BACKEND").is_ok_and(|value| value == "1")
}

fn with_test_restoration<R>(operation: impl FnOnce() -> R) -> R {
    let captured = std::env::var("ZERDR_TEST_FRONTMOST_BEFORE").ok();
    if let Some(identifier) = captured.as_deref() {
        test_log(format!("focus\tcapture {identifier}"));
    }
    let result = operation();
    let current = std::env::var("ZERDR_TEST_FRONTMOST_AFTER").ok();
    if let Some(identifier) = current.as_deref() {
        test_log(format!("focus\tinspect {identifier}"));
    }
    if let (Some(captured), Some(current)) = (captured.as_deref(), current.as_deref())
        && captured != current
        && is_zed_bundle_identifier(current)
        && std::env::var("ZERDR_TEST_FOCUS_ACTIVATION_FAIL").as_deref() != Ok("1")
    {
        test_log(format!("focus\tactivate {captured}"));
    }
    result
}

fn test_log(line: String) {
    let Some(path) = std::env::var_os("ZERDR_TEST_LOG") else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::is_zed_bundle_identifier;

    #[test]
    fn recognizes_only_supported_zed_release_channels() {
        for identifier in [
            "dev.zed.Zed",
            "dev.zed.Zed-Preview",
            "dev.zed.Zed-Nightly",
            "dev.zed.Zed-Dev",
        ] {
            assert!(is_zed_bundle_identifier(identifier));
        }
        assert!(!is_zed_bundle_identifier("dev.zed.Zed-Custom"));
        assert!(!is_zed_bundle_identifier("com.mitchellh.ghostty"));
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2_app_kit::{NSApplicationActivationOptions, NSWorkspace};

    use super::is_zed_bundle_identifier;

    pub(super) fn with_restoration<R>(operation: impl FnOnce() -> R) -> R {
        let workspace = NSWorkspace::sharedWorkspace();
        let captured = workspace.frontmostApplication();
        let result = operation();
        let current = workspace.frontmostApplication();
        let should_restore = captured
            .as_ref()
            .zip(current.as_ref())
            .and_then(|(captured, current)| {
                let current_id = current.bundleIdentifier()?.to_string();
                Some(
                    captured.processIdentifier() != current.processIdentifier()
                        && is_zed_bundle_identifier(&current_id),
                )
            })
            .unwrap_or(false);
        if should_restore && let Some(captured) = captured {
            let _ = captured.activateWithOptions(NSApplicationActivationOptions::empty());
        }
        result
    }
}
