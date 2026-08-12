//! 自動化に与える能力 (ファイル・HTTP)。DESIGN.md 8.5章。
//!
//! 既定では何も許可しない。設定ファイルに書いたものだけが使える。
//! GUIからは編集させない (玄人向け機能であり、誤操作の影響が大きいため)。
//!
//! 方式は「名前付きの窓口」。スクリプトはパスやURLを組み立てられず、
//! 登録済みの名前しか呼べないので、送信先のすり替えや資格情報の持ち出しが起きない。
//! 生パス・生URLのホワイトリストも用意するが既定は空。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;

use anyhow::{Result, bail};
use serde::Deserialize;

/// 許可ディレクトリ内であっても決して触れさせないファイル
/// (自己書き換えと資格情報の吸い出しを防ぐ)
fn is_forbidden(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(name.as_str(), "config.json" | "secrets.json" | ".env") {
        return true;
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 自動化スクリプト自身と暗号化ファイル
    matches!(ext.as_str(), "lua" | "enc")
}

fn rel_is_safe(rel: &str) -> bool {
    let p = Path::new(rel);
    !p.is_absolute()
        && !rel.is_empty()
        && !p
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CapabilitySpec {
    /// 名前付きのファイル窓口
    #[serde(default)]
    pub files: HashMap<String, FileCap>,
    /// 名前付きのHTTP窓口
    #[serde(default)]
    pub http: HashMap<String, HttpCap>,
    /// 玄人向け: 生パスを許可するディレクトリ (既定は空)
    #[serde(default)]
    pub allow_dirs: Vec<String>,
    /// 玄人向け: 生URLを許可するホスト。完全一致で照合する (既定は空)
    #[serde(default)]
    pub allow_hosts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileCap {
    pub dir: String,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpCap {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    /// secretsの tokens から取り出して付与する認証情報の名前
    #[serde(default)]
    pub auth_from_secrets: Option<String>,
    /// 認証情報を載せるヘッダ名 (既定 Authorization)
    #[serde(default = "default_auth_header")]
    pub auth_header: String,
}

fn default_method() -> String {
    "POST".into()
}
fn default_auth_header() -> String {
    "Authorization".into()
}

struct HttpJob {
    url: String,
    method: String,
    body: String,
    auth: Option<(String, String)>,
}

pub struct Capabilities {
    /// 設定から来るもの。読み直しで差し替わる
    spec: std::cell::RefCell<CapabilitySpec>,
    base: PathBuf,
    tokens: std::cell::RefCell<HashMap<String, String>>,
    tx: std::cell::RefCell<Option<mpsc::Sender<HttpJob>>>,
    /// 名前で引くブラウザ。フックは単一スレッドで回るので Rc/RefCell でよい
    /// 帯のボタンが押されたか (名前ごと)。読んだら下ろす
    pressed: std::cell::RefCell<HashMap<String, bool>>,
    /// ターミナルに重ねるか
    /// 窓を持っているなら、その取っ手。ここに置くと別窓にならない
    host: std::cell::RefCell<Option<std::rc::Rc<crate::browser::Browser>>>,
    /// 窓の中で、ブラウザを置く領域
    area: std::cell::Cell<(i32, i32, i32, i32)>,
    /// 窓の中に置いたページの名前。
    ///
    /// 覚えないと、位置を直す先も閉じる先も分からない。
    /// 開いた場所に置きっぱなしになり、窓を動かしても付いてこなかった
    /// 窓の中に置いたページ (持ち主のワークスペース, 呼び名)。
    /// 置いた順を保つので Vec
    hosted: std::cell::RefCell<Vec<(usize, String)>>,
    /// いま見ているワークスペース。呼び名はこの中でだけ通じる
    ws: std::cell::Cell<usize>,
    /// 今どれを、どの領域に出しているか。同じなら送り直さない
    shown: std::cell::RefCell<(Option<String>, (i32, i32, i32, i32))>,
    /// ページの上に出す操作 (名前ごと)。
    ///
    /// 帯と違ってページの中には描かない。ページを一段下げて、
    /// 空いた場所にアプリが描く。だから覚えるのもこちら側で、
    /// 遷移しても消えない
    nav: std::cell::RefCell<HashMap<String, crate::config::NavSpec>>,
    /// 設定に書かれていたから開いたページ (窓の中での名前)。
    ///
    /// 自動化が自分で開いた分や設定画面と区別が要る。設定から消えたときに
    /// 閉じてよいのは、設定が開けたものだけ
    declared: std::cell::RefCell<std::collections::HashSet<String>>,
    /// このワークスペースで使ってよい秘密のキー (既定は空 = 全拒否)。
    /// ラリー(AI製Lua)が別用途の鍵を流用できないよう、ここで絞る
    secret_allow: std::cell::RefCell<std::collections::HashSet<String>>,
    /// 危険承知で全ての秘密を許可する (ワークスペースのトグル)
    secret_allow_all: std::cell::Cell<bool>,
}

/// ページを触るときの既定の待ち時間。
/// 要素の有無を見るだけなので、長くする意味がない
const OP_MS: u64 = 5_000;

impl Capabilities {
    /// 何も許可しない状態 (既定)
    pub fn disabled() -> Self {
        Self {
            spec: std::cell::RefCell::new(CapabilitySpec::default()),
            base: PathBuf::from("."),
            tokens: std::cell::RefCell::new(HashMap::new()),
            tx: std::cell::RefCell::new(None),
            pressed: std::cell::RefCell::new(HashMap::new()),
            host: std::cell::RefCell::new(None),
            area: std::cell::Cell::new((0, 0, 0, 0)),
            hosted: std::cell::RefCell::new(Vec::new()),
            ws: std::cell::Cell::new(0),
            shown: std::cell::RefCell::new((None, (0, 0, 0, 0))),
            nav: std::cell::RefCell::new(HashMap::new()),
            declared: std::cell::RefCell::new(std::collections::HashSet::new()),
            secret_allow: std::cell::RefCell::new(std::collections::HashSet::new()),
            secret_allow_all: std::cell::Cell::new(false),
        }
    }

    pub fn new(spec: CapabilitySpec, base: PathBuf, tokens: HashMap<String, String>) -> Self {
        let me = Self {
            base,
            ..Self::disabled()
        };
        me.set_config(spec, tokens);
        me
    }

    /// 設定から来るものだけを入れ替える。
    ///
    /// 置いたページ・帯・バーは持ち越す。設定を読み直しただけで
    /// 画面から物が消えたり、消えないまま残ったりしてはいけない
    pub fn set_config(&self, spec: CapabilitySpec, tokens: HashMap<String, String>) {
        let wants_http = !spec.http.is_empty() || !spec.allow_hosts.is_empty();
        *self.spec.borrow_mut() = spec;
        *self.tokens.borrow_mut() = tokens;
        if !wants_http || self.tx.borrow().is_some() {
            return;
        }
        *self.tx.borrow_mut() = Some(Self::start_sender());
    }

    /// 通信はUIをブロックしないよう専用スレッドで行う
    fn start_sender() -> mpsc::Sender<HttpJob> {
        {
            let (tx, rx) = mpsc::channel::<HttpJob>();
            std::thread::spawn(move || {
                let agent = ureq::Agent::config_builder()
                    .timeout_global(Some(std::time::Duration::from_secs(15)))
                    .build()
                    .new_agent();
                while let Ok(job) = rx.recv() {
                    // GETは本体を持たないので型が異なる (ureq)
                    let result = match job.method.to_ascii_uppercase().as_str() {
                        "GET" => {
                            let mut r = agent.get(&job.url);
                            if let Some((h, v)) = &job.auth {
                                r = r.header(h.as_str(), v.as_str());
                            }
                            r.call().map(|x| x.status().as_u16())
                        }
                        m => {
                            let mut r = if m == "PUT" {
                                agent.put(&job.url)
                            } else {
                                agent.post(&job.url)
                            };
                            if let Some((h, v)) = &job.auth {
                                r = r.header(h.as_str(), v.as_str());
                            }
                            r.header("Content-Type", "application/json")
                                .send(&job.body)
                                .map(|x| x.status().as_u16())
                        }
                    };
                    match result {
                        Ok(code) => crate::append_hook_log(&format!(
                            "http {} {} -> {code}",
                            job.method, job.url
                        )),
                        Err(e) => {
                            crate::append_hook_log(&format!("http {} 失敗: {e}", job.url))
                        }
                    }
                }
            });
            tx
        }
    }

    /// 名前付き窓口のパスを解決する
    fn named_path(&self, name: &str, rel: &str, want_write: bool) -> Result<PathBuf> {
        let spec = self.spec.borrow();
        let Some(cap) = spec.files.get(name) else {
            bail!("ファイル窓口 '{name}' は未登録です (config.json の capabilities.files)");
        };
        if want_write && !cap.write {
            bail!("'{name}' は書き込みが許可されていません");
        }
        if !want_write && !cap.read {
            bail!("'{name}' は読み取りが許可されていません");
        }
        if !rel_is_safe(rel) {
            bail!("ファイル名が不正です: {rel}");
        }
        let path = self.base.join(&cap.dir).join(rel);
        if is_forbidden(&path) {
            bail!("このファイルは自動化からは扱えません: {rel}");
        }
        Ok(path)
    }

    /// 生パス (allow_dirs 内に限る)
    fn raw_path(&self, p: &str) -> Result<PathBuf> {
        let spec = self.spec.borrow();
        if spec.allow_dirs.is_empty() {
            bail!("生パスの利用は許可されていません (capabilities.allow_dirs が空)");
        }
        let target = self.base.join(p);
        let parent = target.parent().unwrap_or(Path::new("."));
        let canon_parent = parent
            .canonicalize()
            .map_err(|_| anyhow::anyhow!("フォルダが存在しません: {}", parent.display()))?;
        let ok = spec.allow_dirs.iter().any(|d| {
            self.base
                .join(d)
                .canonicalize()
                .map(|c| canon_parent.starts_with(c))
                .unwrap_or(false)
        });
        if !ok {
            bail!("許可されていない場所です: {p}");
        }
        if is_forbidden(&target) {
            bail!("このファイルは自動化からは扱えません: {p}");
        }
        Ok(target)
    }

    pub fn read(&self, name: &str, rel: &str) -> Result<String> {
        let p = self.named_path(name, rel, false)?;
        Ok(std::fs::read_to_string(&p)?)
    }

    pub fn write(&self, name: &str, rel: &str, data: &str) -> Result<()> {
        let p = self.named_path(name, rel, true)?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::crypto::write_atomic(&p, data)?;
        crate::append_hook_log(&format!("write_file {name}/{rel} ({} bytes)", data.len()));
        Ok(())
    }

    pub fn read_raw(&self, p: &str) -> Result<String> {
        Ok(std::fs::read_to_string(self.raw_path(p)?)?)
    }

    pub fn write_raw(&self, p: &str, data: &str) -> Result<()> {
        let path = self.raw_path(p)?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::crypto::write_atomic(&path, data)?;
        crate::append_hook_log(&format!("write_path {p} ({} bytes)", data.len()));
        Ok(())
    }

    /// 名前付きHTTP窓口へ送信する (送りっぱなし。応答はログに残す)
    pub fn http(&self, name: &str, body: &str) -> Result<()> {
        let spec = self.spec.borrow();
        let Some(cap) = spec.http.get(name) else {
            bail!("HTTP窓口 '{name}' は未登録です (config.json の capabilities.http)");
        };
        let auth = cap.auth_from_secrets.as_ref().and_then(|key| {
            self.tokens
                .borrow()
                .get(key)
                .map(|v| (cap.auth_header.clone(), v.clone()))
        });
        self.dispatch(HttpJob {
            url: cap.url.clone(),
            method: cap.method.clone(),
            body: body.to_string(),
            auth,
        })
    }

    /// AIへ渡す文字列から、既知の秘密値を伏字にする (GitHub流のマスク)。
    ///
    /// サンドボックスは秘密の生値を「読む」関数を持たないが、フォームに
    /// 入れた値がDOM経由で読み戻される等の抜け道に備え、AIへ向かうテキスト
    /// (browser の読み取り結果・AIのログ) はここを通してからにする。
    /// 短すぎる値は普通の語まで消してしまうので対象外 (4文字以上)
    pub fn redact(&self, text: &str) -> String {
        let tokens = self.tokens.borrow();
        let mut out = text.to_string();
        for v in tokens.values() {
            if v.trim().chars().count() >= 4 && out.contains(v.as_str()) {
                out = out.replace(v.as_str(), "••••");
            }
        }
        out
    }

    /// このワークスペースで使ってよい秘密を教える (切り替えのたびに呼ぶ)。
    /// 既定は全拒否。all=true は危険承知の全許可トグル
    pub fn set_secret_allow(&self, keys: Vec<String>, all: bool) {
        *self.secret_allow.borrow_mut() = keys.into_iter().collect();
        self.secret_allow_all.set(all);
    }

    /// 秘密の生値を取り出す (Rust内部専用。Luaには決して返さない)。
    ///
    /// 許可リストに無いキーは拒否する。ラリーが別用途の鍵を流用しようとしても、
    /// ここで止まる。値そのものは呼び出し側 (browser_fill_secret 等) が
    /// 認証やフィルに使うだけで、AIの世界には出さない
    pub fn secret_value(&self, key: &str) -> Result<String> {
        if !self.secret_allow_all.get() && !self.secret_allow.borrow().contains(key) {
            bail!("秘密 '{key}' はこのワークスペースで許可されていません");
        }
        self.tokens
            .borrow()
            .get(key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("秘密 '{key}' は未登録です (設定の「秘密」で登録)"))
    }

    /// 窓の中に置く先を教える。設定を読み直すたびに設定し直す
    pub fn set_host(
        &self,
        host: Option<(std::rc::Rc<crate::browser::Browser>, (i32, i32, i32, i32))>,
    ) {
        match host {
            Some((h, area)) => {
                *self.host.borrow_mut() = Some(h);
                self.area.set(area);
            }
            None => *self.host.borrow_mut() = None,
        }
    }

    /// ブラウザを開く (同じ名前があれば、そこで移動する)。
    /// profile はデータ保存の指定 (プロファイル名 / プライベート)
    pub fn browser_open(
        &self,
        name: &str,
        url: &str,
        profile: crate::browser::BrowserProfile,
    ) -> Result<()> {
        // 窓があるなら、その中に置く。別窓にすると位置も重なり順も自前になる
        let host = self
            .host
            .borrow()
            .as_ref()
            .map(std::rc::Rc::clone)
            .ok_or_else(|| anyhow::anyhow!("ブラウザを置く窓がありません"))?;
        let ws = self.ws.get();
        host.open_child(&Self::key(ws, name), url, self.area.get(), profile)?;
        let mut hosted = self.hosted.borrow_mut();
        if !hosted.iter().any(|(w, x)| *w == ws && x == name) {
            hosted.push((ws, name.to_string()));
        }
        // 新しく置いた分は、次の描画で場所を決め直す
        *self.shown.borrow_mut() = (None, (0, 0, 0, 0));
        crate::append_hook_log(&format!("ブラウザ {name}: {url}"));
        Ok(())
    }

    /// 窓の中に置いてあるページの名前 (置いた順)。
    /// そのままタブの並びになる
    pub fn hosted_names(&self) -> Vec<String> {
        let ws = self.ws.get();
        self.hosted
            .borrow()
            .iter()
            .filter(|(w, _)| *w == ws)
            .map(|(_, n)| n.clone())
            .collect()
    }

    /// いま見ているワークスペースを教える。切り替えのたびに呼ぶ
    pub fn set_workspace(&self, ws: usize) {
        self.ws.set(ws);
    }

    /// 窓に置くときの実際の名前。
    /// ワークスペースが違えば、同じ呼び名でも別のページ
    fn key(ws: usize, name: &str) -> String {
        format!("{ws}/{name}")
    }

    /// 中身の領域を教える。窓の大きさが変わるたびに呼ぶ
    pub fn set_area(&self, area: (i32, i32, i32, i32)) {
        self.area.set(area);
    }

    /// 1枚だけを見せて、残りは畳む。
    ///
    /// タブなのだから、見えているのは常に1枚でいい。
    /// 畳むのは幅と高さを0にすること。取り除くと読み込み直しになる。
    ///
    /// 描画は1秒に60回来る。変わっていないなら何も送らない
    pub fn show_only(&self, name: Option<&str>) {
        let want = (name.map(str::to_string), self.area.get());
        if *self.shown.borrow() == want {
            return;
        }
        let Some(h) = self.host.borrow().as_ref().map(std::rc::Rc::clone) else {
            return;
        };
        let ws = self.ws.get();
        for (w, held) in self.hosted.borrow().iter() {
            // 他のワークスペースの分は、生かしたまま畳んでおく
            let r = if *w == ws && Some(held.as_str()) == name {
                want.1
            } else {
                (0, 0, 0, 0)
            };
            let _ = h.child_bounds(&Self::key(*w, held), r);
        }
        *self.shown.borrow_mut() = want;
    }

    /// 名前で1枚を引き当てて、操作を渡す。
    ///
    /// ページは窓の中に置いてある。窓に「このページ宛に」と頼む形になる。
    ///
    /// ここで窓の報告を読んではいけない。報告の線は1本で、
    /// 画面の操作も同じ線に乗っている。横から取ると打鍵やタブの
    /// 切り替えが消える。押された帯は本体のループが受け取り、
    /// note_press で預けてくる
    fn with<T>(
        &self,
        name: &str,
        f: impl FnOnce(&crate::browser::Browser, Option<&str>) -> Result<T>,
    ) -> Result<T> {
        let ws = self.ws.get();
        if !self.hosted.borrow().iter().any(|(w, x)| *w == ws && x == name) {
            return Err(anyhow::anyhow!("ブラウザ '{name}' は開いていません"));
        }
        let host = self
            .host
            .borrow()
            .as_ref()
            .map(std::rc::Rc::clone)
            .ok_or_else(|| anyhow::anyhow!("ブラウザを置く窓がありません"))?;
        f(&host, Some(&Self::key(ws, name)))
    }

    /// 帯のボタンが押されたことを預かる。
    /// 窓の中に置いた分は、本体のループが報告を受け取るのでそこから届く。
    /// 受け取る名前は窓の中での名前 (ワークスペース番号付き)
    pub fn note_press(&self, child: &str) {
        self.pressed.borrow_mut().insert(child.to_string(), true);
    }

    /// 窓の中での名前を、人が使う呼び名へ戻す。
    ///
    /// いま見ているワークスペースのものでなければ None。
    /// 別のワークスペースのページの出来事で、こちらのフックを
    /// 動かすわけにはいかない
    pub fn name_of_child(&self, child: &str) -> Option<String> {
        let head = format!("{}/", self.ws.get());
        child.strip_prefix(&head).map(str::to_string)
    }

    pub fn browser_find(&self, name: &str, sel: &crate::browser::Sel) -> Result<&'static str> {
        self.with(name, |b, to| Ok(b.find(to, sel, OP_MS)?.as_str()))
    }

    pub fn browser_click(&self, name: &str, sel: &crate::browser::Sel) -> Result<&'static str> {
        self.with(name, |b, to| Ok(b.click(to, sel, OP_MS)?.as_str()))
    }

    pub fn browser_fill(
        &self,
        name: &str,
        sel: &crate::browser::Sel,
        value: &str,
    ) -> Result<&'static str> {
        self.with(name, |b, to| Ok(b.fill(to, sel, value, OP_MS)?.as_str()))
    }

    pub fn browser_text(&self, name: &str, sel: &crate::browser::Sel) -> Result<Option<String>> {
        self.with(name, |b, to| b.text(to, sel, OP_MS))
    }

    pub fn browser_html(&self, name: &str) -> Result<String> {
        self.with(name, |b, to| b.html(to, 30_000))
    }

    /// ページの中から通信し、`{status,ok,url,headers,body}` のJSON文字列を返す
    pub fn browser_fetch(&self, name: &str, url: &str, opts: &serde_json::Value) -> Result<String> {
        self.with(name, |b, to| b.fetch(to, url, opts, 30_000))
    }

    /// 人へ呼びかける帯を出す
    pub fn browser_ask(&self, name: &str, text: &str, label: &str) -> Result<()> {
        self.forget_press(name);
        self.with(name, |b, to| b.ask(to, text, label))
    }

    /// 帯のボタンが押されたか。押されていたら下ろして true を返す
    pub fn browser_pressed(&self, name: &str) -> Result<bool> {
        self.with(name, |_, _| Ok(()))?;
        Ok(self.forget_press(name))
    }

    /// 押された記録を下ろす。鍵は窓の中での名前
    fn forget_press(&self, name: &str) -> bool {
        let key = Self::key(self.ws.get(), name);
        self.pressed.borrow_mut().remove(&key).unwrap_or(false)
    }

    pub fn browser_unask(&self, name: &str) -> Result<()> {
        self.forget_press(name);
        self.with(name, |b, to| b.unask(to))
    }

    /// ページの上に操作を出す。出すものが1つも無ければ、出さないのと同じ
    pub fn browser_nav(&self, name: &str, spec: crate::config::NavSpec) -> Result<()> {
        // 開いていないページに出すことはできない。ここで断る
        self.with(name, |_, _| Ok(()))?;
        let key = Self::key(self.ws.get(), name);
        if spec.is_empty() {
            self.nav.borrow_mut().remove(&key);
        } else {
            self.nav.borrow_mut().insert(key, spec);
        }
        Ok(())
    }

    pub fn browser_unnav(&self, name: &str) -> Result<()> {
        self.with(name, |_, _| Ok(()))?;
        self.nav.borrow_mut().remove(&Self::key(self.ws.get(), name));
        Ok(())
    }

    /// ページを動かす。呼び名から窓の中の名前に直すのはこちら側の仕事。
    /// 呼ぶ側に直させると、直し忘れが「何も起きない」として現れる
    pub fn browser_go(&self, name: &str, go: crate::browser::Go) -> Result<()> {
        self.with(name, |b, to| b.go(to, go))
    }

    /// 今どこに居るかを聞く (答えは報告として届く)
    pub fn browser_where(&self, name: &str) -> Result<()> {
        self.with(name, |b, to| b.ask_where(to))
    }

    /// キーボードの焦点をこのページへ移す
    pub fn browser_focus(&self, name: &str) -> Result<()> {
        self.with(name, |b, to| b.focus(to))
    }

    /// このページの画面中継を始める/止める (フレームは本体のループへ報告で届く)
    pub fn browser_screencast(&self, name: &str, on: bool) -> Result<()> {
        self.with(name, |b, to| b.screencast(to, on))
    }

    /// 中継画面へ入力を注入する (指の軌跡・スワイプ・文字)
    pub fn browser_inject(&self, name: &str, input: crate::browser::Input) -> Result<()> {
        self.with(name, |b, to| b.inject(to, input))
    }

    /// ベーシック認証を仕込む。資格情報は秘密 (許可リスト内) から `user:pass` で解決する。
    /// 値はここで解いて CDP に渡すだけで、Lua/AI には一切出さない
    pub fn browser_auth(&self, name: &str, secret_key: &str) -> Result<()> {
        let val = self.secret_value(secret_key)?;
        let (user, pass) = val
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("秘密 '{secret_key}' は user:pass の形式で登録してください"))?;
        self.with(name, |b, to| b.basic_auth(to, user, pass))
    }

    /// 設定に書かれたページを、設定のとおりに揃える。
    ///
    /// 設定から消したページは閉じる。閉じないと、置いたままのページが
    /// 「設定に無いブラウザ」として一覧の後ろに居座り、消したのに
    /// 別の場所に現れる (再起動するまで消えない) ことになる。
    /// 自動化が自分で開いた分と設定画面は、ここでは触らない
    pub fn keep_only_declared(&self, names: &[String]) -> Vec<String> {
        let ws = self.ws.get();
        let want: std::collections::HashSet<String> =
            names.iter().map(|n| Self::key(ws, n)).collect();
        let stale: Vec<String> = self
            .declared
            .borrow()
            .iter()
            .filter(|k| k.starts_with(&format!("{ws}/")) && !want.contains(*k))
            .cloned()
            .collect();
        let mut closed = Vec::new();
        for key in stale {
            self.declared.borrow_mut().remove(&key);
            if let Some(name) = self.name_of_child(&key) {
                if self.browser_close(&name).is_ok() {
                    closed.push(name);
                }
            }
        }
        closed
    }

    /// 設定が開けたページとして控える
    pub fn note_declared(&self, name: &str) {
        self.declared
            .borrow_mut()
            .insert(Self::key(self.ws.get(), name));
    }

    /// 何を出すか (画面を描くループが使う)。
    /// 受けるのは人が使う呼び名。窓の中での名前に直すのはこちらの仕事
    pub fn nav_of(&self, name: &str) -> Option<crate::config::NavSpec> {
        self.nav
            .borrow()
            .get(&Self::key(self.ws.get(), name))
            .copied()
    }

    pub fn browser_close(&self, name: &str) -> Result<()> {
        let ws = self.ws.get();
        let key = Self::key(ws, name);
        if let Some(h) = self.host.borrow().as_ref() {
            let _ = h.unask(Some(&key));
            h.close_child(&key)?;
        }
        self.hosted.borrow_mut().retain(|(w, x)| !(*w == ws && x == name));
        self.pressed.borrow_mut().remove(&key);
        self.nav.borrow_mut().remove(&key);
        self.declared.borrow_mut().remove(&key);
        *self.shown.borrow_mut() = (None, (0, 0, 0, 0));
        Ok(())
    }

    /// 生URL (allow_hosts に完全一致するホストのみ)
    pub fn http_raw(&self, url: &str, body: &str) -> Result<()> {
        let host = host_of(url).ok_or_else(|| anyhow::anyhow!("URLが不正です: {url}"))?;
        if !url.starts_with("https://") {
            bail!("https:// のみ許可されています");
        }
        if !self.spec.borrow().allow_hosts.iter().any(|h| h == &host) {
            bail!("許可されていない接続先です: {host}");
        }
        self.dispatch(HttpJob {
            url: url.to_string(),
            method: "POST".into(),
            body: body.to_string(),
            auth: None,
        })
    }

    fn dispatch(&self, job: HttpJob) -> Result<()> {
        let tx = self.tx.borrow();
        let Some(tx) = tx.as_ref() else {
            bail!("HTTPは有効化されていません");
        };
        tx.send(job)
            .map_err(|_| anyhow::anyhow!("送信キューが閉じています"))
    }
}

#[cfg(test)]
mod reload_tests {
    use super::*;

    /// 設定を読み直しても、窓の中に置いたページを忘れないこと。
    ///
    /// 忘れると、そのページは誰にも動かせないまま画面の上に残る。
    /// 設定画面を保存した瞬間にそれが起き、タブを押しても
    /// 何も変わらないように見えた
    #[test]
    fn reloading_the_settings_does_not_lose_the_pages_on_screen() {
        let c = Capabilities::disabled();
        // 窓の代わりは無いので、置いた記録だけを直に作る
        c.hosted.borrow_mut().push((0, "settings".into()));
        c.nav
            .borrow_mut()
            .insert("0/html".into(), crate::config::NavSpec::all());
        c.note_press("0/html");

        c.set_config(CapabilitySpec::default(), HashMap::new());

        assert_eq!(c.hosted_names(), vec!["settings".to_string()], "置いたページを忘れた");
        assert!(c.nav_of("html").is_some(), "上のバーを忘れた");
        assert!(c.forget_press("html"), "押された帯を忘れた");
    }

    /// 設定から消したブラウザは閉じること。
    ///
    /// 置いたままにすると「設定に無いページ」として一覧の後ろに並び直し、
    /// 消したはずのタブが別の場所に現れる (再起動するまで消えない)
    #[test]
    fn a_browser_removed_from_the_settings_does_not_come_back() {
        let c = Capabilities::disabled();
        for n in ["ai", "html"] {
            c.hosted.borrow_mut().push((0, n.to_string()));
            c.note_declared(n);
        }
        // 自動化が自分で開いた分は、設定に無くても閉じない
        c.hosted.borrow_mut().push((0, "調べ物".into()));

        let closed = c.keep_only_declared(&["ai".to_string()]);
        assert_eq!(closed, vec!["html".to_string()], "消したものが閉じていない");
        assert_eq!(
            c.hosted_names(),
            vec!["ai".to_string(), "調べ物".to_string()],
            "設定が開けていないページまで閉じた"
        );
    }

    /// 設定から来るものは、ちゃんと入れ替わること
    #[test]
    fn the_settings_themselves_are_replaced() {
        let c = Capabilities::disabled();
        assert!(c.read("tmp", "x.txt").is_err(), "何も許していないはず");

        let mut files = HashMap::new();
        files.insert(
            "tmp".to_string(),
            FileCap { dir: ".".into(), read: true, write: false },
        );
        c.set_config(
            CapabilitySpec { files, ..Default::default() },
            HashMap::new(),
        );
        // 窓口は登録された (ファイルが無いので読み込み自体は失敗する)
        let err = c.read("tmp", "居ないファイル.txt").unwrap_err().to_string();
        assert!(!err.contains("未登録"), "窓口が入れ替わっていない: {err}");
    }
}

/// URLからホスト部だけを取り出す (ポート・認証情報は除く)
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let hostport = rest.split(['/', '?', '#']).next()?;
    // user:pass@host のような形は認めない (すり替えを防ぐ)
    if hostport.contains('@') {
        return None;
    }
    let host = hostport.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(spec: CapabilitySpec, base: PathBuf) -> Capabilities {
        Capabilities::new(spec, base, HashMap::new())
    }

    #[test]
    fn redact_masks_known_secret_values_only_when_long_enough() {
        let mut tokens = HashMap::new();
        tokens.insert("diary".to_string(), "hunter2secret".to_string());
        tokens.insert("short".to_string(), "ab".to_string());
        let c = Capabilities::new(CapabilitySpec::default(), PathBuf::from("."), tokens);
        // 既知の秘密値は伏字になる
        let masked = c.redact("Authorization: hunter2secret\n本文");
        assert!(!masked.contains("hunter2secret"), "秘密値が残っている: {masked}");
        assert!(masked.contains("••••"));
        // 短すぎる値は普通の語を壊すので対象外
        assert_eq!(c.redact("ab cd ab"), "ab cd ab");
    }

    #[test]
    fn secret_value_respects_the_allowlist_and_default_denies() {
        let mut tokens = HashMap::new();
        tokens.insert("diary".to_string(), "hunter2secret".to_string());
        tokens.insert("github".to_string(), "ghp_xxx".to_string());
        let c = Capabilities::new(CapabilitySpec::default(), PathBuf::from("."), tokens);
        // 既定は全拒否 (許可リストが空)
        assert!(c.secret_value("diary").is_err(), "既定は全拒否のはず");
        // 許可した鍵だけ取れる
        c.set_secret_allow(vec!["diary".to_string()], false);
        assert_eq!(c.secret_value("diary").unwrap(), "hunter2secret");
        assert!(c.secret_value("github").is_err(), "別用途の鍵は流用できない");
        // 危険承知の全許可トグル
        c.set_secret_allow(vec![], true);
        assert_eq!(c.secret_value("github").unwrap(), "ghp_xxx");
        // 全許可でも、未登録キーは取れない
        assert!(c.secret_value("nope").is_err(), "未登録は取れない");
    }

    #[test]
    fn nothing_is_allowed_by_default() {
        let c = caps(CapabilitySpec::default(), PathBuf::from("."));
        assert!(c.write("reports", "a.md", "x").is_err());
        assert!(c.http("api", "{}").is_err());
        assert!(c.write_raw("a.md", "x").is_err());
    }

    #[test]
    fn named_file_window_confines_writes() {
        let dir = std::env::temp_dir().join("shikisha-caps");
        std::fs::create_dir_all(&dir).unwrap();
        let mut spec = CapabilitySpec::default();
        spec.files.insert(
            "reports".into(),
            FileCap {
                dir: "reports".into(),
                read: true,
                write: true,
            },
        );
        let c = caps(spec, dir.clone());

        assert!(c.write("reports", "ok.md", "hello").is_ok());
        assert_eq!(c.read("reports", "ok.md").unwrap(), "hello");
        // 外へ出ようとする指定は拒否
        assert!(c.write("reports", "../escape.md", "x").is_err());
        assert!(c.write("reports", "C:/windows/x.md", "x").is_err());
        // 未登録の窓口は使えない
        assert!(c.write("other", "a.md", "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crown_jewels_are_never_writable() {
        let dir = std::env::temp_dir().join("shikisha-caps2");
        std::fs::create_dir_all(&dir).unwrap();
        let mut spec = CapabilitySpec::default();
        spec.files.insert(
            "all".into(),
            FileCap {
                dir: ".".into(),
                read: true,
                write: true,
            },
        );
        let c = caps(spec, dir.clone());
        for f in ["config.json", "secrets.json", ".env", "hack.lua", "x.enc"] {
            assert!(c.write("all", f, "x").is_err(), "{f} は拒否されるはず");
            assert!(c.read("all", f).is_err(), "{f} は拒否されるはず");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_matching_is_exact() {
        assert_eq!(host_of("https://api.github.com/x"), Some("api.github.com".into()));
        // 前方一致のすり抜けを許さない
        assert_eq!(
            host_of("https://api.github.com.evil.com/x"),
            Some("api.github.com.evil.com".into())
        );
        // 認証情報つきURLでのすり替えを拒否
        assert_eq!(host_of("https://api.github.com@evil.com/x"), None);

        let mut spec = CapabilitySpec::default();
        spec.allow_hosts.push("api.github.com".into());
        let c = caps(spec, PathBuf::from("."));
        assert!(c.http_raw("https://api.github.com.evil.com/x", "{}").is_err());
        assert!(c.http_raw("http://api.github.com/x", "{}").is_err(), "httpは不可");
    }
}
