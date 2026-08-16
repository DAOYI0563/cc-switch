use std::str::FromStr;

use wsl_code_switch_lib::LegacyAppType;

#[test]
fn parse_known_apps_case_insensitive_and_trim() {
    assert!(matches!(
        LegacyAppType::from_str("claude"),
        Ok(LegacyAppType::Claude)
    ));
    assert!(matches!(
        LegacyAppType::from_str("codex"),
        Ok(LegacyAppType::Codex)
    ));
    assert!(matches!(
        LegacyAppType::from_str("opencode"),
        Ok(LegacyAppType::OpenCode)
    ));
    assert!(matches!(
        LegacyAppType::from_str(" ClAuDe \n"),
        Ok(LegacyAppType::Claude)
    ));
    assert!(matches!(
        LegacyAppType::from_str("\tcoDeX\t"),
        Ok(LegacyAppType::Codex)
    ));
}

#[test]
fn parse_unknown_app_returns_localized_error_message() {
    let err = LegacyAppType::from_str("unknown").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("仅允许"));
    assert!(msg.contains("unknown"));
}
