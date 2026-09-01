//! Locale detection and localized text formatting.
//!
//! Every host-visible string that depends on the user's locale is formatted
//! here: notification action labels, tray timestamps, and relative durations.
//! It exists as its own crate because there are three consumers —
//! `ct-notifications`, the tray menu composition in `ct-runtime`, and the agent
//! — and duplicating the `timeago`/`isolang`/`sys-locale` configuration in each
//! of them already drifted once.

use std::time::{Duration, SystemTime};

/// The user's language, when the platform reports a locale this crate can map.
///
/// Public because callers that format with `timeago` directly still need the
/// same detection, and a second implementation would disagree with this one on
/// locales like `en-GB.UTF-8` or `ru_RU@icu`.
pub fn system_language() -> Option<isolang::Language> {
    system_locale().and_then(|locale| isolang::Language::from_locale(&locale))
}

/// The platform locale, normalized to the `language_TERRITORY` form both
/// `isolang` and `chrono` expect: encoding and modifier suffixes removed, `-`
/// replaced by `_`.
pub fn system_locale() -> Option<String> {
    sys_locale::get_locale().map(|locale| normalize_locale(&locale))
}

fn normalize_locale(locale: &str) -> String {
    locale
        .split(['.', '@'])
        .next()
        .unwrap_or("POSIX")
        .replace('-', "_")
}

/// Formats a duration without a trailing "ago", e.g. `1 hour 1 minute`.
///
/// Used for spans the user chose rather than points in time — the notification
/// "Disable for …" action, for instance.
pub fn human_duration(duration: Duration) -> String {
    // The two branches cannot be merged: `Formatter::new` is generic over a
    // concrete `English`, while `from_isolang` yields a boxed trait object, so
    // the configuration is applied to two different types.
    match system_language().and_then(timeago::from_isolang) {
        Some(language) => {
            let mut formatter = timeago::Formatter::with_language(language);
            formatter.num_items(2).ago("");
            formatter.convert(duration)
        }
        None => {
            let mut formatter = timeago::Formatter::new();
            formatter.num_items(2).ago("");
            formatter.convert(duration)
        }
    }
}

/// Formats how long ago `when` was, e.g. `2 minutes ago`.
///
/// Callers format at the moment the text becomes visible (when a tray menu
/// opens, say), never when the underlying state last changed, or the text is
/// stale by the time it is read.
pub fn relative_time(when: SystemTime) -> String {
    let elapsed = SystemTime::now()
        .duration_since(when)
        .unwrap_or(Duration::ZERO);
    match system_language().and_then(timeago::from_isolang) {
        Some(language) => timeago::Formatter::with_language(language).convert(elapsed),
        None => timeago::Formatter::new().convert(elapsed),
    }
}

/// Formats an absolute local timestamp in the locale's preferred layout.
pub fn absolute_time(when: SystemTime) -> String {
    use chrono::{DateTime, Local, Locale};

    let locale = system_locale()
        .and_then(|locale| locale.parse::<Locale>().ok())
        .unwrap_or(Locale::POSIX);
    DateTime::<Local>::from(when)
        .format_localized("%c", locale)
        .to_string()
}

/// The English singular/plural for rule-count labels shared by notifications
/// and the tray: `1 rule`, any other count `rules`.
pub fn pluralize_rules(count: usize) -> &'static str {
    if count == 1 {
        "rule"
    } else {
        "rules"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_duration_omits_the_ago_suffix() {
        let formatted = human_duration(Duration::from_secs(3_661));

        assert!(!formatted.is_empty());
        assert!(!formatted.ends_with(" ago"), "{formatted}");
    }

    #[test]
    fn locale_normalization_drops_encoding_and_modifiers() {
        assert_eq!(normalize_locale("en-GB.UTF-8"), "en_GB");
        assert_eq!(normalize_locale("ru_RU@icu"), "ru_RU");
        assert_eq!(normalize_locale("de_DE@calendar=gregorian"), "de_DE");
        assert_eq!(normalize_locale(""), "");
    }

    #[test]
    fn absolute_time_is_localized_and_nonempty() {
        assert!(!absolute_time(SystemTime::UNIX_EPOCH).is_empty());
    }

    #[test]
    fn relative_time_formats_a_recent_instant() {
        let formatted = relative_time(SystemTime::now() - Duration::from_secs(120));

        assert!(!formatted.is_empty());
    }

    #[test]
    fn pluralize_rules_distinguishes_the_singular() {
        assert_eq!(pluralize_rules(0), "rules");
        assert_eq!(pluralize_rules(1), "rule");
        assert_eq!(pluralize_rules(2), "rules");
    }
}
