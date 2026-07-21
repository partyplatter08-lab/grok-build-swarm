//! Product identity for the Grok Build Swarm fork vs stock Grok Build.
//!
//! Detected from `argv[0]` so a single binary can be installed as either
//! `grok` or `grok-swarm`. When running as Swarm:
//! - CLI name / about / version prefix say "grok-swarm"
//! - Default leader socket is isolated (`leader-swarm.sock`) so we never
//!   attach to a stock `grok` leader process
//! - Welcome / status chrome shows a Swarm badge

use std::sync::OnceLock;

/// Canonical CLI name for this fork.
pub const SWARM_BIN_NAME: &str = "grok-swarm";
/// Stock CLI name.
pub const STOCK_BIN_NAME: &str = "grok";
/// Product display name (welcome / about).
pub const SWARM_PRODUCT_NAME: &str = "Grok Build Swarm";
/// Stock product display name.
pub const STOCK_PRODUCT_NAME: &str = "Grok Build";
/// Short badge shown in the TUI chrome.
pub const SWARM_BADGE: &str = "⬡ SWARM";
/// Leader socket filename under `~/.grok/` for the Swarm product.
pub const SWARM_LEADER_SOCKET_NAME: &str = "leader-swarm.sock";

/// Resolved product flavor for this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductFlavor {
    Stock,
    Swarm,
}

impl ProductFlavor {
    pub fn is_swarm(self) -> bool {
        matches!(self, Self::Swarm)
    }

    pub fn bin_name(self) -> &'static str {
        match self {
            Self::Stock => STOCK_BIN_NAME,
            Self::Swarm => SWARM_BIN_NAME,
        }
    }

    pub fn product_name(self) -> &'static str {
        match self {
            Self::Stock => STOCK_PRODUCT_NAME,
            Self::Swarm => SWARM_PRODUCT_NAME,
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Self::Stock => "Grok Build TUI",
            Self::Swarm => "Grok Build Swarm — Heavy / Agent Swarm / Swarm Heavy multi-agent modes",
        }
    }

    pub fn badge(self) -> Option<&'static str> {
        match self {
            Self::Stock => None,
            Self::Swarm => Some(SWARM_BADGE),
        }
    }

    /// Prefix for `--version` output (e.g. `grok-swarm 0.2.106 …`).
    pub fn version_prefix(self) -> &'static str {
        self.bin_name()
    }

    /// Hero / splash title (left of version on the welcome panel).
    pub fn splash_title(self) -> &'static str {
        match self {
            Self::Stock => "Grok Build Beta",
            Self::Swarm => "Grok Build Swarm",
        }
    }

    /// Compact product mark used in full version badges (`Grok Build` / `Grok Swarm`).
    pub fn splash_brand(self) -> &'static str {
        match self {
            Self::Stock => "Grok Build",
            Self::Swarm => "Grok Swarm",
        }
    }

    /// Secondary line under the splash title (modes / feedback).
    pub fn splash_subtitle(self) -> &'static str {
        match self {
            Self::Stock => "Thanks for trying Grok Build, give feedback with /feedback!",
            Self::Swarm => {
                "Heavy · Agent Swarm · Swarm Heavy — multi-agent modes · /effort to switch"
            }
        }
    }

    /// Channel / tier suffix next to the brand in full badges (` Beta` / ` Swarm`).
    pub fn splash_channel_suffix(self) -> &'static str {
        match self {
            Self::Stock => " Beta",
            Self::Swarm => " · Multi-agent",
        }
    }
}

/// Detect product from the binary invocation name (argv0).
pub fn detect_from_argv0(argv0: Option<&str>) -> ProductFlavor {
    let name = argv0
        .map(std::path::Path::new)
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or(STOCK_BIN_NAME);
    // Strip common Windows suffixes and versioned names.
    let base = name
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE");
    if base == SWARM_BIN_NAME
        || base.starts_with("grok-swarm-")
        || base.eq_ignore_ascii_case("grokswarm")
    {
        ProductFlavor::Swarm
    } else {
        ProductFlavor::Stock
    }
}

fn flavor_cell() -> &'static OnceLock<ProductFlavor> {
    static FLAVOR: OnceLock<ProductFlavor> = OnceLock::new();
    &FLAVOR
}

/// Initialize product flavor from process args (call once early in main).
pub fn init_from_env() -> ProductFlavor {
    init_with(detect_from_argv0(std::env::args().next().as_deref()))
}

/// Initialize with an explicit flavor (used when the Cargo bin target is
/// `grok-swarm` so renaming the file cannot demote us to stock).
pub fn init_with(flavor: ProductFlavor) -> ProductFlavor {
    let _ = flavor_cell().set(flavor);
    flavor
}

/// Current product flavor (defaults to Stock if never initialized).
pub fn flavor() -> ProductFlavor {
    *flavor_cell().get().unwrap_or(&ProductFlavor::Stock)
}

/// Apply Swarm isolation: dedicated leader socket so we never attach to stock.
///
/// Only sets `GROK_LEADER_SOCKET` when unset, so `--leader-socket` / explicit
/// env still win.
pub fn apply_swarm_isolation(flavor: ProductFlavor) {
    if !flavor.is_swarm() {
        return;
    }
    if std::env::var_os(xai_grok_shell::leader::LEADER_SOCKET_ENV).is_some() {
        return;
    }
    let path = xai_grok_shell::util::grok_home::grok_home().join(SWARM_LEADER_SOCKET_NAME);
    // SAFETY: called once at process start before leader connect.
    unsafe {
        std::env::set_var(
            xai_grok_shell::leader::LEADER_SOCKET_ENV,
            path.as_os_str(),
        );
    }
}

/// System-prompt product label for agent template (`system_prompt_label`).
pub fn system_prompt_label(flavor: ProductFlavor) -> String {
    match flavor {
        ProductFlavor::Stock => "Grok".to_string(),
        ProductFlavor::Swarm => "Grok Build Swarm".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_swarm_from_argv0() {
        assert_eq!(
            detect_from_argv0(Some("/usr/local/bin/grok-swarm")),
            ProductFlavor::Swarm
        );
        assert_eq!(
            detect_from_argv0(Some("grok-swarm-0.2.106-macos-aarch64")),
            ProductFlavor::Swarm
        );
        assert_eq!(
            detect_from_argv0(Some("/Users/me/.grok/bin/grok")),
            ProductFlavor::Stock
        );
        assert_eq!(detect_from_argv0(None), ProductFlavor::Stock);
    }
}
