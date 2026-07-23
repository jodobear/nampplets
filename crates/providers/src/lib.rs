//! Pinned NAP provider implementations and explicit incomplete-domain gates.
//!
//! A type implements [`Provider`] only when the complete pinned domain
//! contract can be fulfilled. Partial implementations deliberately have no
//! descriptor and therefore cannot cross the registry's advertisement
//! boundary.

mod config;
mod shell;
mod storage;
mod theme;

pub use config::{
    ConfigError, ConfigProvider, ConfigProviderLimits, ConfigSchemaErrorCode, SettingsExecutor,
    SettingsExecutorError, SettingsRequest,
};
pub use shell::{
    ShellEnvironment, ShellEnvironmentError, ShellEnvironmentLimits, ShellEnvironmentSource,
    ShellProvider, ShellProviderLimits,
};
pub use storage::{StorageProvider, StorageProviderLimits};
pub use theme::{ThemeProvider, ThemeProviderLimits, ThemeSnapshot, ThemeSource};

/// Compatibility identifier recorded in `compatibility.lock`.
pub const PINNED_NAP_PROTOCOL: &str = "napplet-web@0.28.0";
pub const PINNED_SHELL_PROTOCOL: &str = "NAP-SHELL@6461e4b37c29";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderPushReport {
    pub attempted: usize,
    pub delivered: usize,
    pub refused: usize,
}

impl ProviderPushReport {
    fn record<T, E>(&mut self, result: Result<T, E>) {
        self.attempted = self.attempted.saturating_add(1);
        if result.is_ok() {
            self.delivered = self.delivered.saturating_add(1);
        } else {
            self.refused = self.refused.saturating_add(1);
        }
    }
}
