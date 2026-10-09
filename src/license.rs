//! Pro license check.
//!
//! The account service (shared gateway, milestone M5) will issue an Ed25519-signed license
//! bound to the user and device; the public key is compiled in and the check runs offline.
//! Until that exists nothing unlocks Pro except the free trial sessions, and in debug builds
//! the `LEANFPS_DEV_PRO=1` environment variable for development.

pub fn is_pro(license: Option<&str>) -> bool {
    if cfg!(debug_assertions) && std::env::var("LEANFPS_DEV_PRO").as_deref() == Ok("1") {
        return true;
    }
    // No license format is accepted yet: a value in the config file alone never unlocks Pro.
    let _ = license;
    false
}

/// Where the "Unlock Pro" button sends the user.
pub const BUY_URL: &str = "https://leanfps.com/pro";
