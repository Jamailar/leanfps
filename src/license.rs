//! LeanFPS Pro license: a key bought on leanfps.com, activated once, then checked offline.
//!
//! The license server issues an Ed25519-signed license file bound to this PC. The file is kept as
//! `license.json` in LeanFPS's data folder and verified on every start with the public keys
//! compiled in below, so no network is needed to use Pro. Every few days the file is
//! refreshed in the background; network trouble keeps the current file until it expires,
//! and only a definite answer from the server (refunded, device removed) removes it.

use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

/// License server API for the `leanfps` app.
pub const API_BASE: &str = "https://api.hyperchaos.dev/leanfps/v1";
pub const APP_SLUG: &str = "leanfps";
/// Where the "Unlock Pro" button sends the user.
pub const BUY_URL: &str = "https://leanfps.com/pro/";
/// Find a lost key, see and remove devices.
pub const MANAGE_URL: &str = "https://leanfps.com/license/";
pub const FILE: &str = "license.json";

/// Signing keys of the `leanfps` app (`GET /licenses/public-keys`): kid -> base64url raw
/// Ed25519 public key. Keep every key that ever signed a file; add new ones on rotation.
pub const PUBLIC_KEYS: &[(&str, &str)] = &[("lic-20261010-418e4c", "02oWehuvWg6dDEuOcV02ijClNkTHlWo5BW35liEdFEE")];

const SIGNATURE_DOMAIN: &str = "artisales.app-license.v1\n";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LicenseFile {
    pub payload: Value,
    pub kid: String,
    pub alg: String,
    pub sig: String,
}

/// The fields of a verified payload that LeanFPS uses.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Payload {
    pub app: String,
    pub plan: String,
    pub plan_name: String,
    pub email_hint: String,
    pub device: String,
    pub seats: u32,
    pub max_major: Option<u64>,
    pub refresh_after: u64,
    pub exp: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    None,
    Valid { payload: Payload, needs_refresh: bool },
    Invalid { reason: Invalid, payload: Option<Payload> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    Signature,
    OtherApp,
    OtherDevice,
    Expired,
    VersionNotCovered,
}

impl State {
    pub fn is_pro(&self) -> bool {
        matches!(self, State::Valid { .. })
    }

    pub fn payload(&self) -> Option<&Payload> {
        match self {
            State::Valid { payload, .. } => Some(payload),
            State::Invalid { payload, .. } => payload.as_ref(),
            State::None => None,
        }
    }
}

// ------------------------------------------------------------------ verification

/// The bytes the gateway signs: object keys sorted at every level, no spaces, strings escaped
/// like JavaScript's `JSON.stringify` (which is what serde_json does for strings).
pub fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys.iter().map(|k| format!("{}:{}", Value::String((*k).clone()), canonical_json(&map[*k]))).collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => format!("[{}]", items.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

pub fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn device_hash(fingerprint: &str) -> String {
    sha256_hex(&format!("{APP_SLUG}\n{fingerprint}"))
}

fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The payload when the signature checks out against one of `keys`.
pub fn verify(file: &LicenseFile, keys: &[(&str, &str)]) -> Option<Payload> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    if file.alg != "Ed25519" || !file.payload.is_object() {
        return None;
    }
    let (_, x) = keys.iter().find(|(kid, _)| *kid == file.kid)?;
    let key = VerifyingKey::from_bytes(&base64url_decode(x)?.try_into().ok()?).ok()?;
    let sig = Signature::from_slice(&base64url_decode(&file.sig)?).ok()?;
    let message = format!("{SIGNATURE_DOMAIN}{}", canonical_json(&file.payload));
    key.verify(message.as_bytes(), &sig).ok()?;
    let p = &file.payload;
    if p["v"] != json!(1) || p["typ"] != json!("app-license") {
        return None;
    }
    serde_json::from_value(file.payload.clone()).ok()
}

pub fn major_version(version: &str) -> Option<u64> {
    let digits: String = version.trim().trim_start_matches(['v', 'V']).chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// What LeanFPS may do with a stored file, with no network.
pub fn evaluate(file: Option<&LicenseFile>, keys: &[(&str, &str)], fingerprint: &str, app_version: &str, now: u64) -> State {
    let Some(file) = file else { return State::None };
    let Some(payload) = verify(file, keys) else { return State::Invalid { reason: Invalid::Signature, payload: None } };
    let invalid = |reason| State::Invalid { reason, payload: Some(payload.clone()) };
    if payload.app != APP_SLUG {
        return invalid(Invalid::OtherApp);
    }
    if payload.device != device_hash(fingerprint) {
        return invalid(Invalid::OtherDevice);
    }
    if let (Some(max), Some(major)) = (payload.max_major, major_version(app_version)) {
        if major > max {
            return invalid(Invalid::VersionNotCovered);
        }
    }
    if payload.exp.is_some_and(|exp| now >= exp) {
        return invalid(Invalid::Expired);
    }
    let needs_refresh = now >= payload.refresh_after;
    State::Valid { payload, needs_refresh }
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ------------------------------------------------------------------ server calls

/// Server error with its code (`LICENSE_SEATS_FULL`, ...) and the `errors` detail.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub errors: Value,
}

/// Answers that mean the stored file must stop working (as opposed to "try again later").
///
/// Not `LICENSE_FILE_INVALID`: the stored file already verified against a key compiled into
/// this build, so that answer means the server lost its signing key (a node rebuilt or a
/// database restored), not that the buyer did anything. Not a file the server sent that this
/// build can't check either (a rotated signing key, a PC clock that is far off): both keep the
/// current file working until it expires.
pub fn is_definite_refusal(code: &str) -> bool {
    matches!(code, "LICENSE_REVOKED" | "LICENSE_SUSPENDED" | "LICENSE_DEVICE_REMOVED" | "LICENSE_DEVICE_MISMATCH")
}

/// A file from the server that failed the checks here; `errors` holds why.
pub const FILE_UNVERIFIED: &str = "FILE_UNVERIFIED";
/// `license.json` could not be written.
pub const SAVE_FAILED: &str = "SAVE_FAILED";

impl ApiError {
    pub fn network(detail: String) -> ApiError {
        ApiError { status: 0, code: "NETWORK".into(), errors: Value::String(detail) }
    }

    /// What to tell the user. The gateway's own messages are Chinese, so map the codes.
    pub fn message(&self) -> String {
        match self.code.as_str() {
            "NETWORK" => "Could not reach the LeanFPS license server. Check your internet connection and try again.",
            "LICENSE_NOT_FOUND" => "That key and email don't match a license. Check both: use the email you bought with.",
            "LICENSE_SEATS_FULL" => "This key is already active on all of its PCs. Remove one below, then activate again.",
            "LICENSE_VERSION_NOT_COVERED" => "This key doesn't cover this version of LeanFPS.",
            "LICENSE_DEVICE_CHANGES_EXCEEDED" => "This key moved between PCs too often this month. Email contact@leanfps.com and we'll reset it.",
            "LICENSE_REVOKED" => "This license was refunded or replaced and no longer works.",
            "LICENSE_SUSPENDED" => "This license is suspended. Email contact@leanfps.com.",
            "LICENSE_DEVICE_REMOVED" | "LICENSE_DEVICE_MISMATCH" => "This PC was removed from the license. Activate it again with your key.",
            "LICENSE_TOO_MANY_ATTEMPTS" => "Too many attempts. Wait an hour and try again.",
            "LICENSE_FILE_INVALID" => "The license server couldn't read this PC's license. Enter your key again, or email contact@leanfps.com.",
            FILE_UNVERIFIED if self.errors == json!("clock") => "Your PC's date and time look wrong. Set them correctly in Windows Settings, then try again.",
            FILE_UNVERIFIED => "The license server sent a license this version of LeanFPS can't check. Update LeanFPS and try again.",
            SAVE_FAILED => "LeanFPS couldn't save the license on this PC. Restart LeanFPS and try again.",
            _ if self.status == 429 => "Too many attempts. Wait a few minutes and try again.",
            _ if self.status >= 500 => "The license server had a problem. Try again in a few minutes.",
            _ => "Activation failed. Check the key and email, or email contact@leanfps.com.",
        }
        .to_string()
    }

    /// Devices listed with `LICENSE_SEATS_FULL`.
    pub fn devices(&self) -> Vec<Device> {
        self.errors.get("devices").and_then(|d| serde_json::from_value(d.clone()).ok()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Device {
    pub id: String,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub current: bool,
}

/// POST a JSON body, return (HTTP status, body). Implemented with WinHTTP on Windows.
pub type Transport = fn(url: &str, body: &str) -> Result<(u16, String), String>;

pub fn call(transport: Transport, route: &str, body: &Value) -> Result<Value, ApiError> {
    let (status, text) = transport(&format!("{API_BASE}/licenses/{route}"), &body.to_string()).map_err(ApiError::network)?;
    let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(data);
    }
    let code = match &data["code"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => status.to_string(),
    };
    Err(ApiError { status, code, errors: data["errors"].clone() })
}

/// Identity of this PC sent on activation; the server only stores a salted hash of it.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub fingerprint: String,
    pub name: String,
    pub os_version: String,
}

pub struct Licenses {
    pub store: Store,
    pub transport: Transport,
    pub keys: &'static [(&'static str, &'static str)],
    pub device: DeviceInfo,
    pub app_version: &'static str,
    pub now: fn() -> u64,
}

impl Licenses {
    pub fn file(&self) -> Option<LicenseFile> {
        self.store.load(FILE)
    }

    pub fn state(&self) -> State {
        evaluate(self.file().as_ref(), self.keys, &self.device.fingerprint, self.app_version, (self.now)())
    }

    fn accept(&self, data: &Value) -> Result<State, ApiError> {
        let unverified = |why: &str| ApiError { status: 0, code: FILE_UNVERIFIED.into(), errors: json!(why) };
        let file: LicenseFile = serde_json::from_value(data["license_file"].clone()).map_err(|_| unverified("format"))?;
        // Never trust the network blindly: the file must verify against the pinned keys and
        // belong to this PC before it replaces anything.
        let state = evaluate(Some(&file), self.keys, &self.device.fingerprint, self.app_version, (self.now)());
        match &state {
            State::Valid { .. } => {}
            // Just signed by the server, yet already expired here: this PC's clock is ahead.
            State::Invalid { reason: Invalid::Expired, .. } => return Err(unverified("clock")),
            State::Invalid { reason, .. } => return Err(unverified(&format!("{reason:?}"))),
            State::None => return Err(unverified("format")),
        }
        self.store.save(FILE, &file).map_err(|e| ApiError { status: 0, code: SAVE_FAILED.into(), errors: Value::String(e) })?;
        Ok(state)
    }

    pub fn activate(&self, key: &str, email: &str) -> Result<State, ApiError> {
        let body = json!({
            "license_key": key.trim(),
            "email": email.trim(),
            "app_version": self.app_version,
            "device": { "fingerprint": self.device.fingerprint, "name": self.device.name, "platform": "windows", "os_version": self.device.os_version },
        });
        let data = call(self.transport, "activate", &body)?;
        self.accept(&data)
    }

    /// Refresh when the file asks for it (or `force`). Network trouble keeps the current
    /// file; a definite refusal removes it and returns the error.
    pub fn refresh_if_due(&self, force: bool) -> Result<State, ApiError> {
        let current = self.state();
        let due = match &current {
            State::Valid { needs_refresh, .. } => *needs_refresh || force,
            State::Invalid { reason: Invalid::Expired, .. } => true,
            _ => false,
        };
        let Some(file) = self.file().filter(|_| due) else { return Ok(current) };
        let body = json!({ "license_file": file, "app_version": self.app_version, "device": { "fingerprint": self.device.fingerprint } });
        match call(self.transport, "validate", &body).and_then(|data| self.accept(&data)) {
            Ok(state) => Ok(state),
            Err(e) if is_definite_refusal(&e.code) => {
                self.store.remove(FILE);
                Err(e)
            }
            Err(_) => Ok(self.state()),
        }
    }

    /// Frees this PC's seat, then forgets the file even when the server can't be reached.
    pub fn deactivate(&self) -> Result<(), ApiError> {
        let result = match self.file() {
            Some(file) => call(self.transport, "deactivate", &json!({ "license_file": file, "device": { "fingerprint": self.device.fingerprint } })).map(|_| ()),
            None => Ok(()),
        };
        self.store.remove(FILE);
        result
    }

    pub fn remove_device(&self, key: &str, email: &str, activation_id: &str) -> Result<(), ApiError> {
        call(self.transport, "devices/remove", &json!({ "license_key": key.trim(), "email": email.trim(), "activation_id": activation_id })).map(|_| ())
    }
}

/// `win:{MachineGuid}`: stable across reinstalling LeanFPS, so a reinstall never takes a new seat.
pub fn machine_fingerprint(pf: &dyn crate::platform::Platform) -> String {
    use crate::catalog::RegData;
    if let Ok(Some(RegData::Sz(id))) = pf.reg_get("HKLM\\SOFTWARE\\Microsoft\\Cryptography", "MachineGuid") {
        if !id.trim().is_empty() {
            return format!("win:{}", id.trim());
        }
    }
    if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
        if !id.trim().is_empty() {
            return format!("linux:{}", id.trim());
        }
    }
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    format!("host:{}", sha256_hex(&format!("{host}|{}", std::env::var("USERNAME").unwrap_or_default())))
}

pub fn device_info(pf: &dyn crate::platform::Platform) -> DeviceInfo {
    let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Windows PC".into());
    let build = pf.machine().os_build;
    DeviceInfo { fingerprint: machine_fingerprint(pf), name, os_version: if build > 0 { format!("10.0.{build}") } else { String::new() } }
}

/// Debug builds only: `LEANFPS_DEV_PRO=1` unlocks Pro for UI work without a key.
pub fn dev_pro() -> bool {
    cfg!(debug_assertions) && std::env::var("LEANFPS_DEV_PRO").as_deref() == Ok("1")
}

/// HTTPS POST for the license server.
pub fn transport() -> Transport {
    #[cfg(windows)]
    {
        crate::platform::win::https_post_json
    }
    #[cfg(not(windows))]
    {
        curl_post_json
    }
}

/// Development on Linux only (the product is Windows-only): the system `curl`.
#[cfg(not(windows))]
fn curl_post_json(url: &str, body: &str) -> Result<(u16, String), String> {
    let out = std::process::Command::new("curl")
        .args(["-sS", "--max-time", "20", "-X", "POST", "-H", "Content-Type: application/json", "--data-binary", body, "-w", "\n%{http_code}", url])
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n').ok_or("no response")?;
    Ok((code.trim().parse().map_err(|_| "no response")?, body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Signed by Node's crypto with the gateway's canonical JSON (see tools/license_fixture.mjs).
    const KID: &str = "lic-test";
    const PUBLIC: &str = include_str!("../tools/license_fixture.pub").trim_ascii();
    const FIXTURE: &str = include_str!("../tools/license_fixture.json");
    const FINGERPRINT: &str = "win:11111111-2222-3333-4444-555555555555";
    const IAT: u64 = 1_791_504_000;

    fn keys() -> Vec<(&'static str, &'static str)> {
        vec![(KID, PUBLIC)]
    }

    fn fixture() -> LicenseFile {
        serde_json::from_str(FIXTURE).unwrap()
    }

    #[test]
    fn canonical_json_sorts_keys_and_escapes_like_javascript() {
        let v = json!({ "b": 1, "a": { "z": null, "y": [true, "é\n\"x\""] } });
        assert_eq!(canonical_json(&v), r#"{"a":{"y":[true,"é\n\"x\""],"z":null},"b":1}"#);
    }

    #[test]
    fn base64url_round_trip() {
        assert_eq!(base64url_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64url_decode("-_8").unwrap(), [0xfb, 0xff]);
        assert!(base64url_decode("a b").is_none());
    }

    #[test]
    fn verifies_a_gateway_signed_file() {
        let keys = keys();
        let file = fixture();
        let p = verify(&file, &keys).expect("signature verifies");
        assert_eq!(p.app, "leanfps");
        assert_eq!(p.device, device_hash(FINGERPRINT));
        let iat = IAT;
        assert_eq!(file.payload["iat"].as_u64(), Some(IAT));
        match evaluate(Some(&file), &keys, FINGERPRINT, "0.1.0", iat + 60) {
            State::Valid { needs_refresh, .. } => assert!(!needs_refresh),
            s => panic!("{s:?}"),
        }
        let after = p.refresh_after + 1;
        assert!(matches!(evaluate(Some(&file), &keys, FINGERPRINT, "0.1.0", after), State::Valid { needs_refresh: true, .. }));
        assert!(matches!(evaluate(Some(&file), &keys, FINGERPRINT, "0.1.0", p.exp.unwrap()), State::Invalid { reason: Invalid::Expired, .. }));
        assert!(matches!(evaluate(Some(&file), &keys, "win:other", "0.1.0", iat), State::Invalid { reason: Invalid::OtherDevice, .. }));
        assert!(matches!(evaluate(Some(&file), &keys, FINGERPRINT, "2.0.0", iat), State::Invalid { reason: Invalid::VersionNotCovered, .. }));
        assert!(matches!(evaluate(Some(&file), &[], FINGERPRINT, "0.1.0", iat), State::Invalid { reason: Invalid::Signature, .. }));
        assert_eq!(evaluate(None, &keys, FINGERPRINT, "0.1.0", iat), State::None);
    }

    #[test]
    fn tampered_files_are_rejected() {
        let keys = keys();
        let mut file = fixture();
        file.payload["seats"] = json!(99);
        assert!(verify(&file, &keys).is_none());
        let mut file = fixture();
        file.sig = file.sig.replacen(|c: char| c.is_ascii_alphabetic(), "0", 1);
        assert!(verify(&file, &keys).is_none());
        let mut file = fixture();
        file.alg = "HS256".into();
        assert!(verify(&file, &keys).is_none());
    }

    #[test]
    fn versions() {
        assert_eq!(major_version("0.1.0"), Some(0));
        assert_eq!(major_version("v12.3"), Some(12));
        assert_eq!(major_version("beta"), None);
    }

    #[test]
    fn errors_map_to_english_and_list_devices() {
        let e = ApiError { status: 409, code: "LICENSE_SEATS_FULL".into(), errors: json!({ "devices": [{ "id": "a1", "device_name": "DESKTOP-1", "current": false }] }) };
        assert!(e.message().contains("Remove one"));
        assert_eq!(e.devices()[0].device_name.as_deref(), Some("DESKTOP-1"));
        assert!(is_definite_refusal("LICENSE_REVOKED") && !is_definite_refusal("NETWORK"));
    }

    // A fake server keeps the network out of the tests.
    thread_local! {
        static REPLY: std::cell::RefCell<(u16, String)> = std::cell::RefCell::new((200, String::new()));
    }
    fn fake(_url: &str, _body: &str) -> Result<(u16, String), String> {
        Ok(REPLY.with(|r| r.borrow().clone()))
    }
    fn offline(_url: &str, _body: &str) -> Result<(u16, String), String> {
        Err("offline".into())
    }

    fn licenses(transport: Transport) -> Licenses {
        let keys: &'static [(&'static str, &'static str)] = Box::leak(keys().into_boxed_slice());
        Licenses {
            store: Store::temp(),
            transport,
            keys,
            device: DeviceInfo { fingerprint: FINGERPRINT.into(), name: "PC".into(), os_version: "10.0.26100".into() },
            app_version: "0.1.0",
            now: || IAT + 60,
        }
    }

    #[test]
    fn activation_saves_only_a_verified_file_for_this_pc() {
        let l = licenses(fake);
        REPLY.with(|r| *r.borrow_mut() = (200, json!({ "license_file": fixture() }).to_string()));
        assert!(l.activate("LFPS-AAAAA", "a@b.c").unwrap().is_pro());
        assert_eq!(l.file(), Some(fixture()));

        let mut other = fixture();
        other.payload["seats"] = json!(5);
        REPLY.with(|r| *r.borrow_mut() = (200, json!({ "license_file": other }).to_string()));
        let l = licenses(fake);
        assert_eq!(l.activate("k", "e").unwrap_err().code, FILE_UNVERIFIED);
        assert!(l.file().is_none(), "an unverified file is never written");

        REPLY.with(|r| *r.borrow_mut() = (403, json!({ "code": "LICENSE_NOT_FOUND", "message": "授权码或邮箱不对" }).to_string()));
        assert_eq!(l.activate("k", "e").unwrap_err().code, "LICENSE_NOT_FOUND");
    }

    #[test]
    fn refresh_keeps_the_file_offline_and_drops_it_on_refusal() {
        let l = licenses(offline);
        l.store.save(FILE, &fixture()).unwrap();
        assert!(l.refresh_if_due(true).unwrap().is_pro());
        assert!(l.file().is_some(), "network trouble never removes the license");

        let l = licenses(fake);
        l.store.save(FILE, &fixture()).unwrap();
        REPLY.with(|r| *r.borrow_mut() = (403, json!({ "code": "LICENSE_REVOKED" }).to_string()));
        assert!(l.refresh_if_due(false).unwrap().is_pro(), "not due yet: no call");
        assert_eq!(l.refresh_if_due(true).unwrap_err().code, "LICENSE_REVOKED");
        assert!(l.file().is_none());

        let l = licenses(fake);
        l.store.save(FILE, &fixture()).unwrap();
        REPLY.with(|r| *r.borrow_mut() = (503, String::new()));
        assert!(l.refresh_if_due(true).unwrap().is_pro(), "server trouble keeps the license");
    }

    #[test]
    fn refresh_keeps_the_file_when_the_server_cannot_be_trusted_or_checked() {
        // The server lost its signing key and no longer recognises the file.
        let l = licenses(fake);
        l.store.save(FILE, &fixture()).unwrap();
        REPLY.with(|r| *r.borrow_mut() = (400, json!({ "code": "LICENSE_FILE_INVALID" }).to_string()));
        assert!(l.refresh_if_due(true).unwrap().is_pro());
        assert_eq!(l.file(), Some(fixture()));

        // The server rotated its signing key: the new file has a kid this build doesn't know.
        let mut rotated = fixture();
        rotated.kid = "lic-20991231-000000".into();
        REPLY.with(|r| *r.borrow_mut() = (200, json!({ "license_file": rotated }).to_string()));
        assert!(l.refresh_if_due(true).unwrap().is_pro());
        assert_eq!(l.file(), Some(fixture()), "the old file keeps working until it expires");
    }

    #[test]
    fn a_clock_far_ahead_is_named_on_activation() {
        let mut l = licenses(fake);
        l.now = || IAT + 400 * 86_400;
        REPLY.with(|r| *r.borrow_mut() = (200, json!({ "license_file": fixture() }).to_string()));
        let e = l.activate("k", "e").unwrap_err();
        assert_eq!(e.code, FILE_UNVERIFIED);
        assert!(e.message().contains("date and time"), "{}", e.message());
        assert!(l.file().is_none());
    }

    /// WinHTTP end to end against the real license server: TLS, proxy, POST body, status and
    /// JSON error body. Runs on the Windows CI runner.
    #[cfg(windows)]
    #[test]
    fn winhttp_reaches_the_license_server() {
        let (status, body) = crate::platform::win::https_post_json(&format!("{API_BASE}/licenses/validate"), "{}").expect("network");
        assert!((400..500).contains(&status), "status {status}: {body}");
        let v: Value = serde_json::from_str(&body).expect("JSON error body");
        assert!(v.get("code").is_some(), "{body}");
    }

    /// The whole flow against the live license server with a real key and the shipped public
    /// keys: activate, refresh, deactivate. Run by hand with a test key:
    /// `LEANFPS_TEST_KEY=... LEANFPS_TEST_EMAIL=... cargo test live_license_round_trip -- --ignored`
    #[test]
    #[ignore]
    fn live_license_round_trip() {
        let (key, email) = (std::env::var("LEANFPS_TEST_KEY").unwrap(), std::env::var("LEANFPS_TEST_EMAIL").unwrap());
        let l = Licenses {
            store: Store::temp(),
            transport: transport(),
            keys: PUBLIC_KEYS,
            device: DeviceInfo { fingerprint: format!("test:{}", now_secs()), name: "LeanFPS live test".into(), os_version: "10.0.26100".into() },
            app_version: env!("CARGO_PKG_VERSION"),
            now: now_secs,
        };
        let state = l.activate(&key, &email).expect("activate");
        let p = state.payload().unwrap().clone();
        assert!(state.is_pro() && p.app == APP_SLUG && p.plan == "pro", "{state:?}");
        assert_eq!(l.state(), state, "the saved file verifies offline");
        // Same PC again (a reinstall) reuses its seat.
        assert!(l.activate(&key, &email).unwrap().is_pro());
        assert!(l.refresh_if_due(true).expect("refresh").is_pro());
        assert_eq!(l.deactivate(), Ok(()));
        assert_eq!(l.state(), State::None);
    }

    #[test]
    fn deactivate_forgets_the_file_even_offline() {
        let l = licenses(offline);
        l.store.save(FILE, &fixture()).unwrap();
        assert!(l.deactivate().is_err());
        assert!(l.file().is_none());
    }
}
