pub mod email;
pub mod google;
pub mod password;
pub mod session;

/// Whitelist of trusted avatar-image hosts. Centralized here so the JSON
/// (`PATCH /me`) and HTML (`POST /profile`) update paths can't drift to
/// different lists. Add a host by editing one constant.
const ALLOWED_AVATAR_PREFIXES: &[&str] = &[
    "https://lh3.googleusercontent.com/",
    "https://avatars.githubusercontent.com/",
];

/// `true` iff `url` is hosted on one of the trusted avatar CDNs. Empty
/// strings should be checked separately by the caller (we don't decide
/// "no avatar at all" policy here).
pub fn is_allowed_avatar_url(url: &str) -> bool {
    ALLOWED_AVATAR_PREFIXES.iter().any(|prefix| url.starts_with(prefix))
}
