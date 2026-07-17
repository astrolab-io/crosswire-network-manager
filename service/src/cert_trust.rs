// SPDX-License-Identifier: GPL-3.0-or-later
//! Detecting crosswire's "gateway certificate not trusted" failure.
//!
//! crosswire pins the gateway's leaf certificate by SHA-256 (`--trusted-cert`).
//! When the gateway rotates its certificate the pin no longer matches and
//! crosswire aborts during the TLS handshake — *before* the SAML/login step,
//! which is why the desktop never sees a browser prompt. On that failure it
//! prints the presented leaf's digest. We watch its output for that specific
//! error and recover the new digest so the service can offer the user a native
//! "trust this certificate?" prompt (and, once accepted, re-pin it).
//!
//! This is deliberately coupled to crosswire's human-readable output rather than
//! a machine protocol: the alternative — re-probing the gateway's TLS ourselves
//! — would duplicate crosswire's trust logic (chain validation + pin set) and
//! could disagree with it. Reading crosswire's own verdict keeps it the single
//! source of truth and adds no TLS dependency here.

/// Substring that marks crosswire's cert-rejection error (its `bail!` message,
/// surfaced on stderr as `Error: gateway certificate not trusted`).
const UNTRUSTED_MARKER: &str = "gateway certificate not trusted";

/// Accumulates evidence, across crosswire's output lines, of a rejected gateway
/// certificate and the digest it presented. Fed every stdout/stderr line of one
/// crosswire run; the two facts can arrive on either stream, in either order.
#[derive(Debug, Default)]
pub struct CertScanner {
    untrusted: bool,
    digest: Option<String>,
}

impl CertScanner {
    /// Feed one line of crosswire output (order-independent).
    pub fn observe(&mut self, line: &str) {
        if line.contains(UNTRUSTED_MARKER) {
            self.untrusted = true;
        }
        // crosswire prints the digest as `sha256 digest: <hex>` and again in its
        // `--trusted-cert <hex>` hint; only look on those lines so an unrelated
        // 64-hex token elsewhere can't be mistaken for the presented leaf.
        if self.digest.is_none()
            && (line.contains("digest") || line.contains("trusted-cert"))
            && let Some(d) = extract_sha256(line)
        {
            self.digest = Some(d);
        }
    }

    /// The presented leaf digest iff crosswire rejected the gateway certificate
    /// *and* we recovered a digest — the two facts a trust prompt needs. Absent
    /// either, this is `None` and the failure is handled as an ordinary one.
    pub fn cert_change(&self) -> Option<&str> {
        if self.untrusted {
            self.digest.as_deref()
        } else {
            None
        }
    }
}

/// Extract the first 64-character lowercase-hex token (a SHA-256 digest) from a
/// line, if any. crosswire lowercases its digests (`{:02x}`), and the pin
/// comparison is an exact string match, so we require lowercase to reproduce the
/// exact value crosswire would accept.
fn extract_sha256(line: &str) -> Option<String> {
    line.split(|c: char| !c.is_ascii_hexdigit())
        .find(|tok| {
            tok.len() == 64
                && tok
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .map(str::to_string)
}

/// What to do when the crosswire child exits unexpectedly during connect.
#[derive(Debug, PartialEq, Eq)]
pub enum ExitAction {
    /// Re-launch in place (a transient fast connect-phase failure).
    Retry,
    /// The gateway certificate changed; prompt the user to trust `digest`
    /// instead of retrying (retrying can't fix a changed cert).
    PromptCert(String),
    /// Give up and report `ConnectFailed` to NetworkManager.
    Fail,
}

/// Decide how to handle an unexpected crosswire exit. A detected cert change
/// (only meaningful while still connecting) short-circuits the retry logic —
/// re-launching would just re-reject the same certificate.
pub fn classify_exit(
    cert_change: Option<&str>,
    fast: bool,
    connecting: bool,
    retries: u32,
    max_retries: u32,
) -> ExitAction {
    if connecting
        && let Some(digest) = cert_change
    {
        return ExitAction::PromptCert(digest.to_string());
    }
    if retries < max_retries && fast && connecting {
        return ExitAction::Retry;
    }
    ExitAction::Fail
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "e34e0d64da8ad0f6cb47a66e6bc9100a2d6ac3636455e12e89dd8663e0f5cf96";

    #[test]
    fn detects_untrusted_with_digest_line() {
        let mut s = CertScanner::default();
        // Real-shape lines: the tracing digest (stdout) + the anyhow error (stderr).
        s.observe(&format!(
            "2026-07-17T17:31:35Z ERROR crosswire::transport::tls:   sha256 digest: {DIGEST}"
        ));
        s.observe("Error: gateway certificate not trusted");
        assert_eq!(s.cert_change(), Some(DIGEST));
    }

    #[test]
    fn recovers_digest_from_trusted_cert_hint() {
        let mut s = CertScanner::default();
        s.observe(&format!(
            "  If you trust it, rerun with: --trusted-cert {DIGEST}"
        ));
        s.observe("gateway certificate not trusted");
        assert_eq!(s.cert_change(), Some(DIGEST));
    }

    #[test]
    fn order_independent() {
        // Marker first, then digest.
        let mut s = CertScanner::default();
        s.observe("Error: gateway certificate not trusted");
        s.observe(&format!("  sha256 digest: {DIGEST}"));
        assert_eq!(s.cert_change(), Some(DIGEST));
    }

    #[test]
    fn no_change_without_untrusted_marker() {
        let mut s = CertScanner::default();
        // A digest with no rejection is just informational, not a cert change.
        s.observe(&format!("  sha256 digest: {DIGEST}"));
        assert_eq!(s.cert_change(), None);
    }

    #[test]
    fn no_change_without_digest() {
        let mut s = CertScanner::default();
        s.observe("Error: gateway certificate not trusted");
        assert_eq!(s.cert_change(), None);
    }

    #[test]
    fn ignores_non_digest_tokens() {
        // Too short, wrong context, and uppercase (crosswire emits lowercase).
        assert_eq!(extract_sha256("deadbeef digest"), None);
        assert_eq!(extract_sha256("nothing hex here"), None);
        assert_eq!(
            extract_sha256(
                "digest: E34E0D64DA8AD0F6CB47A66E6BC9100A2D6AC3636455E12E89DD8663E0F5CF96"
            ),
            None
        );
    }

    #[test]
    fn classify_prompts_on_cert_change_before_retrying() {
        // Even with retries available, a cert change goes straight to the prompt.
        assert_eq!(
            classify_exit(Some(DIGEST), true, true, 0, 2),
            ExitAction::PromptCert(DIGEST.to_string())
        );
    }

    #[test]
    fn classify_retries_fast_connect_failure() {
        assert_eq!(classify_exit(None, true, true, 0, 2), ExitAction::Retry);
        // Exhausted retries → fail.
        assert_eq!(classify_exit(None, true, true, 2, 2), ExitAction::Fail);
        // Slow failure (past the window) or not connecting → fail, no retry.
        assert_eq!(classify_exit(None, false, true, 0, 2), ExitAction::Fail);
        assert_eq!(classify_exit(None, true, false, 0, 2), ExitAction::Fail);
    }

    #[test]
    fn classify_ignores_cert_change_once_started() {
        // A cert change can only occur pre-tunnel; if we're no longer connecting
        // it isn't actionable as a prompt.
        assert_eq!(classify_exit(Some(DIGEST), true, false, 0, 2), ExitAction::Fail);
    }
}
