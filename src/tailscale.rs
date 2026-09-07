//! The HTTPS address Tailscale is putting in front of ours.
//!
//! The phone link is an `http://100.x.y.z:8787/…` address. Over Tailscale that
//! is already encrypted and already reachable only by this person's own
//! devices, so nothing about it is unsafe. It is, however, **not a secure
//! context** as a browser counts them, and three things a browser will only do
//! in one are things this program wants:
//!
//!   - installing the page on a home screen (Chrome refuses the manifest
//!     outright over plain http, whatever the address)
//!   - push notifications, later
//!   - the clipboard, the camera, and everything else behind that same gate
//!
//! `tailscale serve` solves it in one command. It terminates TLS on a real
//! certificate for `<machine>.<tailnet>.ts.net` and proxies to a local port,
//! and it stays inside the tailnet — the public version is a different verb
//! (`funnel`), which is why this file refuses to adopt one.
//!
//! ## Nothing here is assumed
//!
//! A link this program hands out is the only way back to the board from a
//! train. Handing out one that does not work is worse than handing out a plain
//! one, so the HTTPS address is adopted only when **both** of these hold:
//!
//!   1. Tailscale says a serve rule proxies to our own port. The port is what
//!      makes this exact rather than probable: two copies of this program
//!      cannot both hold one port on one machine, so a rule naming our port
//!      names us.
//!   2. A request through that address comes back. Whether the rule points at
//!      the loopback or at the tailnet address, whether the certificate is
//!      there yet, whether MagicDNS resolves — none of that is worth reasoning
//!      about when it can be tried.
//!
//! Anything else, including no Tailscale at all, leaves the plain address in
//! place. That address has always worked and still does.

use std::time::Duration;

/// Where the `tailscale` command lives.
///
/// The installer's own path first, because `PATH` on Windows frequently does
/// not have it: the GUI is what people install, and it does not put the CLI
/// anywhere a shell will find it.
fn cli() -> Option<String> {
    let installed = r"C:\Program Files\Tailscale\tailscale.exe";
    if std::path::Path::new(installed).exists() {
        return Some(installed.to_string());
    }
    // Falls back to the name, for a machine where it is on PATH (and for
    // anything that is not Windows).
    Some("tailscale".to_string())
}

/// The host and port a serve handler proxies to, out of `http://host:port/…`.
fn target_of(proxy: &str) -> Option<(String, u16)> {
    let after = proxy.split("://").nth(1).unwrap_or(proxy);
    let hostport = after.split('/').next()?;
    let (host, port) = hostport.rsplit_once(':')?;
    Some((host.to_string(), port.parse().ok()?))
}

/// The address a serve rule would put in front of `port`, before it is proved.
///
/// Split out from `front` so the parsing can be tested against real output
/// without a Tailscale on the machine running the tests.
fn declared(serve_json: &str, port: u16) -> Option<String> {
    let cfg: serde_json::Value = serde_json::from_str(serve_json).ok()?;
    let web = cfg.get("Web")?.as_object()?;
    for (hostport, entry) in web {
        let handlers = entry.get("Handlers").and_then(|h| h.as_object());
        let ours = handlers.is_some_and(|hs| {
            hs.iter().any(|(path, h)| {
                // Only the root. A rule mounted at /something else is somebody
                // else's arrangement, and the link this program hands out is a
                // link to the root.
                path == "/"
                    && h.get("Proxy")
                        .and_then(|p| p.as_str())
                        .and_then(target_of)
                        .is_some_and(|(_, p)| p == port)
            })
        });
        if !ours {
            continue;
        }
        // Funnel is the same rule opened to the whole internet, and it is
        // refused outright -- not gated behind a setting.
        //
        // What is being decided here is which address goes into a QR code with
        // a token beside it, under a badge that says who can reach it. A
        // `.ts.net` name that is funnelled looks exactly like one that is not,
        // so adopting one would mean either a badge that lies or a second
        // opinion about what counts as private, and this program already knows
        // how that ends. Nothing is broken by refusing: the funnel still works
        // for anyone who types it, and the private address this falls back to
        // is the one that has always been handed out.
        let funnelled = cfg
            .get("AllowFunnel")
            .and_then(|f| f.get(hostport.as_str()))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if funnelled {
            crate::append_hook_log(&format!(
                "tailscale: {hostport} is on the public internet (funnel); \
                 the phone link stays on the private address"
            ));
            return None;
        }
        let (host, hp) = hostport.rsplit_once(':')?;
        return Some(match hp {
            "443" => format!("https://{host}"),
            other => format!("https://{host}:{other}"),
        });
    }
    None
}

/// Ask this address for something, to find out whether it is really there.
fn answers(origin: &str) -> bool {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .new_agent();
    // The manifest, because it is the one route that needs no token and no
    // session -- exactly the thing a browser fetches on its own account.
    match agent.get(&format!("{origin}{}", crate::pwa::MANIFEST_PATH)).call() {
        Ok(r) => r.status().as_u16() == 200,
        Err(_) => false,
    }
}

/// The HTTPS origin in front of `port`, or `None` to keep the plain one.
pub fn front(port: u16) -> Option<String> {
    let exe = cli()?;
    // Reusing the short-command runner from the other file that asks this
    // machine what it has: same shape of question, same need for a window that
    // never flashes up and a command that cannot hang the start-up.
    // Two seconds is generous for what it is: a question asked of a daemon on
    // this same machine, which answers in milliseconds or not at all. The QR
    // waits for this, so a Tailscale that is installed but not running must
    // not be able to hold the phone card up for long.
    let out = crate::discover::run_briefly(&exe, &["serve", "status", "--json"], Duration::from_secs(2))?;
    let origin = declared(&String::from_utf8_lossy(&out), port)?;
    if !answers(&origin) {
        crate::append_hook_log(&format!(
            "tailscale: {origin} is configured for this port but did not answer; \
             the phone link stays on the plain address"
        ));
        return None;
    }
    crate::append_hook_log(&format!("tailscale: the phone link is {origin}"));
    Some(origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output, from `tailscale serve status --json`.
    const SERVED: &str = r#"{
      "TCP": { "443": { "HTTPS": true } },
      "Web": {
        "ipc.tail4871ca.ts.net:443": {
          "Handlers": { "/": { "Proxy": "http://127.0.0.1:8787" } }
        }
      }
    }"#;

    #[test]
    fn the_rule_has_to_name_our_own_port() {
        assert_eq!(
            declared(SERVED, 8787).as_deref(),
            Some("https://ipc.tail4871ca.ts.net")
        );
        // Somebody else's server behind the same name is not ours to advertise
        assert_eq!(declared(SERVED, 8788), None);
        assert_eq!(declared("{}", 8787), None);
        assert_eq!(declared("not json", 8787), None);
    }

    /// A rule mounted somewhere other than the root leads somewhere other than
    /// the board.
    #[test]
    fn only_the_root_counts() {
        let sub = SERVED.replace("\"/\"", "\"/other\"");
        assert_eq!(declared(&sub, 8787), None);
    }

    /// A funnelled name is the same address on the open internet, and it is
    /// indistinguishable from a private one by looking at it. So it is never
    /// the address that gets handed out.
    #[test]
    fn a_funnelled_name_is_never_handed_out() {
        let public = SERVED.replace(
            "\"TCP\":",
            "\"AllowFunnel\": { \"ipc.tail4871ca.ts.net:443\": true }, \"TCP\":",
        );
        assert_eq!(declared(&public, 8787), None, "公開されたものは配らない");
    }

    /// A serve rule on a port other than 443 keeps its port in the link.
    #[test]
    fn a_port_that_is_not_the_usual_one_stays_in_the_address() {
        let odd = SERVED.replace("ts.net:443", "ts.net:8443");
        assert_eq!(
            declared(&odd, 8787).as_deref(),
            Some("https://ipc.tail4871ca.ts.net:8443")
        );
    }

    /// What this machine actually says, right now.
    ///
    /// Ignored by default and run by hand (`cargo test -- --ignored
    /// what_this_machine_says --nocapture`): the answer depends on how the
    /// machine is set up, so it cannot assert anything. It is here because the
    /// two ways this can come out -- no rule for our port, or a rule that does
    /// not answer -- both end in the same quiet `None`, and when the phone link
    /// is not what somebody expected this prints which of the two it was.
    #[test]
    #[ignore]
    fn what_this_machine_says() {
        match cli().and_then(|exe| {
            crate::discover::run_briefly(&exe, &["serve", "status", "--json"], Duration::from_secs(4))
        }) {
            Some(out) => println!("serve status:
{}", String::from_utf8_lossy(&out)),
            None => println!("no tailscale here"),
        }
        for port in [8787u16, 8765] {
            println!("port {port}: {:?}", front(port));
        }
    }

    /// The whole thing, through a real `tailscale serve`, on this machine.
    ///
    /// Ignored by default and run by hand with the port an existing serve rule
    /// already points at:
    ///
    ///   `cargo test -- --ignored through_a_real_serve --nocapture`
    ///
    /// It stands a board up on that port and then asks, exactly as start-up
    /// does, where the board is reached. A pass means the certificate, the
    /// name, the proxy and our own token-free manifest route all lined up --
    /// which is the one thing no amount of parsing can tell you.
    #[test]
    #[ignore]
    fn through_a_real_serve() {
        // Whatever the machine's own rule proxies to. Nothing to set up, and
        // nothing left changed afterwards.
        let Some(port) = cli()
            .and_then(|exe| {
                crate::discover::run_briefly(&exe, &["serve", "status", "--json"], Duration::from_secs(4))
            })
            .and_then(|out| {
                let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out)).ok()?;
                v.get("Web")?.as_object()?.values().find_map(|e| {
                    e.get("Handlers")?.as_object()?.get("/")?.get("Proxy")?.as_str().and_then(target_of)
                })
            })
            .map(|(_, p)| p)
        else {
            println!("no serve rule on this machine -- nothing to prove against");
            return;
        };
        println!("standing a board up on the port the rule points at: {port}");
        let ui = crate::remote::RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            port,
            "board-token-0000".into(),
            String::new(),
        )
        .expect("bind");
        let found = front(ui.port());
        println!("reached at: {found:?}");
        ui.shutdown();
        assert_eq!(
            found.as_deref().map(|o| o.starts_with("https://")),
            Some(true),
            "serve が立っているのに HTTPS の入口が見つからない"
        );
    }

    #[test]
    fn a_proxy_target_is_read_down_to_its_port() {
        assert_eq!(target_of("http://127.0.0.1:8787"), Some(("127.0.0.1".into(), 8787)));
        assert_eq!(target_of("http://100.115.38.97:8787/"), Some(("100.115.38.97".into(), 8787)));
        assert_eq!(target_of("http://localhost:80/x"), Some(("localhost".into(), 80)));
        assert_eq!(target_of("http://nowhere"), None);
    }
}
