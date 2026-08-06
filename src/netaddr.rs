//! 待ち受けアドレスの決定。DESIGN.md 10.4章。
//!
//! リモートUIは「遠隔からAIに任意のコマンドを実行させられる」機能なので、
//! どこに公開するかを厳しく扱う。プライベート網 (Tailscale / LAN) 以外への
//! バインドは、設定で明示的に許可しない限り拒否する。

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// そのアドレスへ到達するときに使う自分側のIPを調べる。
/// 実際には送信しない (UDPのconnectは経路を決めるだけ) ので副作用がない
fn local_ip_for(target: &str) -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect(target).ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_unspecified() && !v4.is_loopback() => Some(v4),
        _ => None,
    }
}

/// Tailscaleのアドレス (100.64.0.0/10)。MagicDNSの解決先への経路から判定する
pub fn tailscale_ip() -> Option<Ipv4Addr> {
    let ip = local_ip_for("100.100.100.100:80")?;
    is_tailscale(&ip).then_some(ip)
}

/// LAN内のアドレス (192.168.x / 10.x / 172.16-31.x)
pub fn lan_ip() -> Option<Ipv4Addr> {
    let ip = local_ip_for("8.8.8.8:80")?;
    ip.is_private().then_some(ip)
}

pub fn is_tailscale(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..128).contains(&o[1])
}

/// 外に晒しても比較的安全な範囲か (ループバック・LAN・Tailscale)
pub fn is_private(ip: &Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private() || ip.is_link_local() || is_tailscale(ip)
}

/// 設定の bind 指定を実際のアドレスに解決する。
/// "auto" は Tailscale → LAN → ループバック の順に探す
pub fn resolve_bind(spec: &str, allow_public: bool) -> Result<(Ipv4Addr, Option<String>), String> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("auto") {
        if let Some(ip) = tailscale_ip() {
            return Ok((ip, None));
        }
        if let Some(ip) = lan_ip() {
            return Ok((ip, Some(crate::i18n::t("remote.err.lan_only"))));
        }
        return Err(crate::i18n::t("remote.err.no_network"));
    }
    let ip: Ipv4Addr = spec
        .parse()
        .map_err(|_| crate::i18n::tp("remote.err.bad_address", &[("addr", spec)]))?;
    if !is_private(&ip) && !allow_public {
        return Err(crate::i18n::tp("remote.err.public", &[("ip", &ip.to_string())]));
    }
    Ok((ip, None))
}

/// 設定画面に表示するQRコード (SVG)。スマホのカメラで読ませる
pub fn qr_svg(text: &str, scale: u32) -> String {
    use qrcode::{EcLevel, QrCode};
    let Ok(code) = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L) else {
        return String::new();
    };
    let w = code.width() as u32;
    let quiet = 4;
    let size = (w + quiet * 2) * scale;
    let dark: Vec<bool> = code
        .into_colors()
        .iter()
        .map(|c| *c == qrcode::Color::Dark)
        .collect();
    let mut rects = String::new();
    for y in 0..w {
        for x in 0..w {
            if dark[(y * w + x) as usize] {
                let px = (x + quiet) * scale;
                let py = (y + quiet) * scale;
                rects.push_str(&format!(
                    r#"<rect x="{px}" y="{py}" width="{scale}" height="{scale}"/>"#
                ));
            }
        }
    }
    // 色指定の "#..." が終端と衝突しないよう r##"..."## で囲む
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {size} {size}"><rect width="{size}" height="{size}" fill="#fff"/><g fill="#000">{rects}</g></svg>"##
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ranges_are_recognized() {
        assert!(is_tailscale(&"100.101.102.103".parse().unwrap()));
        assert!(!is_tailscale(&"100.200.1.1".parse().unwrap()), "100.128以降は別");
        assert!(is_private(&"192.168.1.5".parse().unwrap()));
        assert!(is_private(&"10.0.0.1".parse().unwrap()));
        assert!(is_private(&"127.0.0.1".parse().unwrap()));
        assert!(!is_private(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn public_bind_requires_explicit_permission() {
        // 公開アドレスは既定で拒否する (遠隔実行を晒さない)
        let e = resolve_bind("203.0.113.5", false).unwrap_err();
        assert!(e.contains("allow_public"), "{e}");
        // 明示的に許可すれば通す
        assert!(resolve_bind("203.0.113.5", true).is_ok());
        // プライベートは既定で通る
        assert_eq!(
            resolve_bind("192.168.1.5", false).unwrap().0,
            "192.168.1.5".parse::<Ipv4Addr>().unwrap()
        );
        assert!(resolve_bind("なにこれ", false).is_err());
    }

    /// 実環境で何が選ばれるかを確認する (環境依存なので結果は表示のみ)
    #[test]
    fn auto_bind_picks_a_private_address_when_available() {
        let ts = tailscale_ip();
        let lan = lan_ip();
        println!("tailscale={ts:?} lan={lan:?}");
        if let Ok((ip, note)) = resolve_bind("auto", false) {
            assert!(is_private(&ip), "自動選択は必ずプライベート網: {ip}");
            if ts.is_some() {
                assert_eq!(Some(ip), ts, "Tailscaleがあればそれを優先する");
                assert!(note.is_none(), "Tailscaleなら注意書きは不要");
            }
        }
    }

    #[test]
    fn qr_svg_is_scannable_markup() {
        let svg = qr_svg("http://100.64.0.1:8787/?t=abc", 6);
        assert!(svg.starts_with("<svg"), "SVGとして返す");
        assert!(svg.contains("<rect"), "モジュールが描かれている");
        assert!(svg.matches("<rect").count() > 50, "十分な数のモジュール");
    }


}
