//! Cross-language rules run against their actual browser implementations.
use serde_json::json;

fn node(name: &str, script: &str) -> serde_json::Value {
    let dir = crate::test_temp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("check.cjs");
    std::fs::write(&path, script).unwrap();
    let result = std::process::Command::new("node")
        .arg(&path)
        .output()
        .expect("Node is required for browser contract checks");
    let _ = std::fs::remove_file(path);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

#[test]
fn browser_and_store_agree_on_secret_urls() {
    let mut cases = vec![
        "",
        "example.com",
        "https://",
        "https://:443",
        "https://[::1]",
        "https://*.com",
        "https://*.[",
        "https://user@site/path",
        "http://local/",
        " HTTPS://example.com ",
        "https://a/b*c",
        "https://a/?x=*",
        "https://a/#*",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    for scheme in ["http", "https", "HTTP", "ftp", "x"] {
        for host in [
            "example.com",
            "*.example.com",
            "*.co.jp",
            "*a.com",
            "a.*.com",
            "a:b",
            "",
            "[::1]",
            "local",
        ] {
            for port in [
                "", ":0", ":443", ":65535", ":65536", ":99999", ":0000443", ":+443", ":", ":-1",
                ":1.0",
            ] {
                cases.push(format!("{scheme}://{host}{port}/path?q=1#end"));
            }
        }
    }
    for space in [
        ' ', '\t', '\n', '\u{85}', '\u{a0}', '\u{2003}', '\u{2028}', '\u{feff}',
    ] {
        cases.push(format!("{space}https://example.com{space}"));
        cases.push(format!("https://a{space}b.com"));
    }
    let script = include_str!("secret-url.js").replace(
        "{{SECRET_URL_POLICY}}",
        &crate::config::secret_url_policy_json(),
    );
    let actual = node(
        "url-contract",
        &format!(
            "{script}\nconsole.log(JSON.stringify({}.map(urlFault)));",
            json!(cases)
        ),
    );
    for (text, browser) in cases.iter().zip(actual.as_array().unwrap()) {
        assert_eq!(
            *browser,
            json!(crate::config::url_fault(text)),
            "browser and store disagree: {text:?}"
        );
    }
    assert_eq!(
        crate::config::url_fault("https://example.com:99999"),
        Some("err.secret_url.unreadable")
    );
}

#[test]
fn interpolation_keeps_values_literal_in_both_languages() {
    let args = [
        ("name", "{why}"),
        ("why", "Denied"),
        ("jp", "日本語"),
        ("x-y", "$&"),
    ];
    let cases = [
        "{name}: {why}",
        "{name}/{name}",
        "{missing}",
        "{jp} {x-y}",
        "{{why}}",
        "{}",
        "x{unclosed",
        "{name}{why}{name}",
        "toString: {toString}",
    ];
    let values: serde_json::Value = args
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    let actual = node(
        "fill-contract",
        &format!(
            "{}\nconsole.log(JSON.stringify({}.map(s => fill(s, {}))));",
            crate::i18n::FILL_JS,
            json!(cases),
            values
        ),
    );
    for (text, browser) in cases.iter().zip(actual.as_array().unwrap()) {
        assert_eq!(*browser, json!(crate::i18n::fill(text, &args)), "{text}");
    }
    assert_eq!(crate::i18n::fill(cases[0], &args), "{why}: Denied");
}

#[test]
fn settings_transport_preserves_raw_responses_and_reports_failures() {
    let script = format!(
        r#"
const assert = require('node:assert/strict');
const TOKEN = 'fixture';
const calls = [];
let reply;
const fetch = async (path, options) => {{ calls.push({{path, options}}); if (reply instanceof Error) throw reply; return reply; }};
{}
(async () => {{
  reply = {{ok:true, json:async () => ({{ok:true}})}};
  assert.deepEqual(await settingsApi('/read'), {{ok:true}});
  assert.equal(calls.at(-1).options.method, 'GET');
  assert.equal(calls.at(-1).options.headers['X-Token'], TOKEN);
  await settingsApi('/save', {{value:'日本語'}});
  assert.equal(calls.at(-1).options.body, '{{"value":"日本語"}}');
  assert.equal(calls.at(-1).options.headers['Content-Type'], 'application/json');
  assert.equal(await settingsFetch('/download', {{method:'POST', body:'raw', headers:{{Accept:'text/plain'}}}}), reply);
  assert.equal(calls.at(-1).options.body, 'raw');
  assert.equal(calls.at(-1).options.headers.Accept, 'text/plain');
  reply = {{ok:false, status:403, json:async () => ({{error:'Refused'}})}};
  await assert.rejects(settingsApi('/denied'), /Refused/);
  assert.deepEqual(await postJson('/denied', {{}}), {{ok:false}});
  reply = new Error('Offline');
  await assert.rejects(settingsApi('/offline'), /Offline/);
  assert.deepEqual(await postJson('/offline', {{}}), {{ok:false}});
  console.log('true');
}})().catch(e => {{ console.error(e); process.exitCode = 1; }});
"#,
        include_str!("settings-api.js")
    );
    assert_eq!(node("settings-transport", &script), json!(true));
}
