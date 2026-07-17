// SPDX-License-Identifier: GPL-3.0-or-later
//! Launching a helper GUI in the logged-in user's graphical session.
//!
//! The service runs as root on the system bus. A native dialog, though, must run
//! as the human at the keyboard — their display, their `libnm`/polkit identity
//! (so it may edit and re-activate *their* connection). We identify that user
//! authoritatively from systemd-logind's active local graphical session — which
//! works even though we were D-Bus-activated by NetworkManager with no login
//! ancestry to infer it from — then start the helper inside their systemd user
//! manager. This mirrors how crosswire opens the SSO browser (`net/browser.rs`).

use std::process::{Command, Stdio};

/// Start `program args...` detached in the active local graphical session's
/// user. Returns `false` if there is no such session (e.g. headless, or the
/// screen is locked to the greeter), so the caller can fall back to plain
/// failure. Best-effort and non-blocking: we do not wait for the dialog.
pub fn spawn_in_user_session(program: &str, args: &[String]) -> bool {
    let Some(uid) = active_graphical_uid() else {
        return false;
    };
    let Some(user) = uid_name(uid) else {
        return false;
    };

    // Preferred: the user's systemd manager (every systemd desktop). `--user
    // --machine <user>@.host` enters their session bus/environment; without
    // `--wait` this returns as soon as the transient unit is started.
    if command_exists("systemd-run") {
        let started = Command::new("systemd-run")
            .args([
                "--quiet",
                "--user",
                "--machine",
                &format!("{user}@.host"),
                "--",
            ])
            .arg(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if started {
            return true;
        }
    }

    // Fallback (non-systemd user sessions): run it directly as the user.
    let cmdline = std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .map(|a| shell_single_quote(&a))
        .collect::<Vec<_>>()
        .join(" ");
    Command::new("su")
        .args([user.as_str(), "-c", &cmdline])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Owner uid of the active, local, graphical logind session — the authoritative
/// "who is at the keyboard", independent of process ancestry.
fn active_graphical_uid() -> Option<u32> {
    let list = Command::new("loginctl")
        .args(["list-sessions", "--no-legend"])
        .output()
        .ok()?;
    if !list.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&list.stdout).lines() {
        let Some(id) = line.split_whitespace().next() else {
            continue;
        };
        let show = Command::new("loginctl")
            .args([
                "show-session",
                id,
                "-p",
                "Active",
                "-p",
                "Remote",
                "-p",
                "Type",
                "-p",
                "User",
            ])
            .output()
            .ok()?;
        if show.status.success()
            && let Some(uid) = parse_graphical_session_uid(&String::from_utf8_lossy(&show.stdout))
        {
            return Some(uid);
        }
    }
    None
}

/// From `loginctl show-session` `Key=Value` output, return the owner uid iff the
/// session is active, local, and graphical (i.e. a dialog can open in it).
fn parse_graphical_session_uid(props: &str) -> Option<u32> {
    let (mut active, mut remote, mut graphical, mut uid) = (false, false, false, None);
    for line in props.lines() {
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        match key {
            "Active" => active = val == "yes",
            "Remote" => remote = val == "yes",
            "Type" => graphical = matches!(val, "wayland" | "x11" | "mir"),
            "User" => uid = val.trim().parse::<u32>().ok(),
            _ => {}
        }
    }
    if active && !remote && graphical {
        uid
    } else {
        None
    }
}

/// Resolve a uid to its login name via the passwd database.
fn uid_name(uid: u32) -> Option<String> {
    // getpwuid_r without extra crates: shell out to `getent`, present anywhere
    // logind is. Falls back to `id -nu` if getent is unavailable.
    let out = Command::new("getent")
        .args(["passwd", &uid.to_string()])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    if let Some(line) = out
        && let Some(name) = line.split(':').next()
        && !name.is_empty()
    {
        return Some(name.to_string());
    }
    Command::new("id")
        .args(["-nu", &uid.to_string()])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn command_exists(name: &str) -> bool {
    std::env::var("PATH")
        .ok()
        .map(|p| {
            p.split(':')
                .any(|d| std::path::Path::new(d).join(name).is_file())
        })
        .unwrap_or(false)
}

/// POSIX single-quote a shell word: wrap in `'…'`, and encode embedded quotes as
/// `'\''`. Only used for the non-systemd `su -c` fallback.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::{parse_graphical_session_uid, shell_single_quote};

    #[test]
    fn picks_active_local_graphical_session() {
        assert_eq!(
            parse_graphical_session_uid("Active=yes\nRemote=no\nType=wayland\nUser=1000\n"),
            Some(1000)
        );
        // Property order doesn't matter; x11 counts too.
        assert_eq!(
            parse_graphical_session_uid("User=1001\nType=x11\nActive=yes\nRemote=no"),
            Some(1001)
        );
    }

    #[test]
    fn rejects_inactive_remote_or_nongraphical() {
        assert_eq!(
            parse_graphical_session_uid("Active=no\nRemote=no\nType=wayland\nUser=1000"),
            None
        );
        assert_eq!(
            parse_graphical_session_uid("Active=yes\nRemote=no\nType=tty\nUser=1000"),
            None
        );
        assert_eq!(
            parse_graphical_session_uid("Active=yes\nRemote=yes\nType=x11\nUser=1000"),
            None
        );
    }

    #[test]
    fn single_quotes_escape_embedded_quotes() {
        assert_eq!(shell_single_quote("abc"), "'abc'");
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }
}
