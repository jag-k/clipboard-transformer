#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupportLevel {
    Native,
    BestEffort,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformCapabilities {
    pub clipboard: SupportLevel,
    pub notifications: SupportLevel,
    pub tray: SupportLevel,
    pub autostart: SupportLevel,
    pub source_app_metadata: SupportLevel,
}

impl PlatformCapabilities {
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                clipboard: SupportLevel::Native,
                notifications: SupportLevel::Native,
                tray: SupportLevel::Native,
                autostart: SupportLevel::Native,
                source_app_metadata: SupportLevel::BestEffort,
            }
        }
        #[cfg(target_os = "windows")]
        {
            Self {
                clipboard: SupportLevel::Native,
                notifications: SupportLevel::BestEffort,
                tray: SupportLevel::Native,
                autostart: SupportLevel::Native,
                source_app_metadata: SupportLevel::BestEffort,
            }
        }
        #[cfg(target_os = "linux")]
        {
            // Keep the public support matrix unavailable until the packaged
            // desktop runtime passes real X11/XWayland and Wayland session
            // tests. Session diagnostics below this coarse matrix already
            // report the concrete backend discovered at runtime.
            Self {
                clipboard: SupportLevel::Unavailable,
                notifications: SupportLevel::Unavailable,
                tray: SupportLevel::Unavailable,
                autostart: SupportLevel::Unavailable,
                source_app_metadata: SupportLevel::Unavailable,
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Self {
                clipboard: SupportLevel::Unavailable,
                notifications: SupportLevel::Unavailable,
                tray: SupportLevel::Unavailable,
                autostart: SupportLevel::Unavailable,
                source_app_metadata: SupportLevel::Unavailable,
            }
        }
    }

    pub const fn runtime_available(self) -> bool {
        matches!(self.clipboard, SupportLevel::Native)
    }
}

impl std::fmt::Display for SupportLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Native => formatter.write_str("native"),
            Self::BestEffort => formatter.write_str("best-effort"),
            Self::Unavailable => formatter.write_str("unavailable"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_requires_a_real_clipboard_backend() {
        let unavailable = PlatformCapabilities {
            clipboard: SupportLevel::Unavailable,
            notifications: SupportLevel::Native,
            tray: SupportLevel::Native,
            autostart: SupportLevel::Native,
            source_app_metadata: SupportLevel::BestEffort,
        };
        assert!(!unavailable.runtime_available());
    }
}
