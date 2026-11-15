//! `graphersal-memory ui`: the browser UI of the memory (the `memory` profile of Graphersal's
//! playground, `crate::ui_assets`), for every user.
//!
//! A small HTTP server on `127.0.0.1:<random port>` (never another address), a client of the
//! daemon like the shim: it is the network boundary, the daemon stays socket-only.
//!
//! - **Token**: 256 random bits per run (`ring`), required as `Authorization: Bearer <token>`
//!   on every `/api/` request (never a cookie or a query parameter), compared in constant time,
//!   never logged. Static files carry no data and need none.
//! - **Host and Origin**: every request passes graphersal-serve's `HostCheck` for loopback and
//!   this port (`Host` exactly `127.0.0.1:<port>`, an `Origin` when present
//!   `http://127.0.0.1:<port>`), so a page of another site cannot reach it through DNS
//!   rebinding. No `Access-Control-*` header is ever sent; writes are `POST` only.
//! - **Opening the browser**: through a launch file `ui-<pid>.html` in the run directory (mode
//!   0600 in the 0700 directory; on Windows the per-user run directory), a redirect to
//!   `http://127.0.0.1:<port>/#token=<token>`; the browser is started with the FILE's path only
//!   (the token never shows in a process list). The page moves the token into
//!   `sessionStorage` and clears the address (`ui/boot.js`). The file is removed at the page's
//!   first heartbeat and when `ui` exits; a launch file older than the heartbeat timeout is
//!   removed by the next `ui` start (a `ui` ended by Ctrl+C before a page opened leaves one).
//! - **Lifetime**: foreground until Ctrl+C, or until no page sent a heartbeat (`POST
//!   /api/ui/heartbeat`, every 60 s from each tab) for [`HEARTBEAT_TIMEOUT`]. One open
//!   connection to the daemon (a `use` handshake without traffic) keeps the daemon alive while
//!   `ui` runs; it reconnects when the daemon restarted (killed, crashed, upgraded), starting a
//!   new one as the shim does, and so do the relayed requests and the start context (unless a
//!   daemon was stopped on purpose by `uninstall` or `purge`: `crate::shim::may_start`).
//! - **Answers**: `/api/ui/*` itself ([`ui_api`]: status, projects, start context, the start
//!   context budget, heartbeat), every other `/api/` path relayed to the daemon
//!   (`crate::relay`), static files from the profile's own files first
//!   ([`crate::ui_assets::FILES_PREFIX`]), then graphersal-serve's embedded playground (its
//!   `index.html` with the profile's boot script added).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use graphersal_serve::{Exposure, HostCheck};
use serde_json::{Value, json};

use crate::shim::DaemonLocation;

/// How long `ui` runs without a heartbeat of any page (user decision 5b, 2026-11-13).
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The launch files' name prefix in the run directory.
pub const LAUNCH_PREFIX: &str = "ui-";

/// How long the status and doctor lines are reused before they are read again.
const REPORT_FOR: Duration = Duration::from_secs(30);

/// The largest `memory.start_context_chars` the UI sets.
pub const MAX_START_CONTEXT_CHARS: u64 = 1_000_000;

/// What `ui` serves.
pub struct UiOptions {
    /// The daemon (started when none runs).
    pub location: DaemonLocation,
    /// The port to listen on (`0`: a free one).
    pub port: u16,
    /// Whether to open the browser.
    pub open_browser: bool,
    /// Exit after this long without a heartbeat.
    pub heartbeat_timeout: Duration,
    /// The `status` and `doctor` lines (plain text, no colour): `{"status": [..], "doctor":
    /// [..]}`.
    pub report: Arc<dyn Fn() -> Value + Send + Sync>,
}

/// One HTTP answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The status.
    pub status: u16,
    /// The headers (besides the ones every answer gets, [`common_headers`]).
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl Reply {
    fn json(status: u16, value: &Value) -> Self {
        Reply {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: value.to_string().into_bytes(),
        }
    }

    fn error(status: u16, message: &str) -> Self {
        Self::json(status, &json!({"error": message}))
    }
}

/// The headers of every answer: no caching, no sniffing, no referrer, no framing. Never an
/// `Access-Control-*` header.
pub fn common_headers() -> [(&'static str, &'static str); 4] {
    [
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("X-Frame-Options", "DENY"),
    ]
}

/// The state the request threads share.
pub struct Shared {
    token: String,
    port: u16,
    check: HostCheck,
    location: DaemonLocation,
    last_heartbeat: Mutex<Instant>,
    launch_file: PathBuf,
    launch_removed: AtomicBool,
    report: Arc<dyn Fn() -> Value + Send + Sync>,
    report_cache: Mutex<Option<(Instant, Value)>>,
}

impl Shared {
    /// The shared state of a `ui` on `port` with `token`.
    pub fn new(
        token: String,
        port: u16,
        location: DaemonLocation,
        launch_file: PathBuf,
        report: Arc<dyn Fn() -> Value + Send + Sync>,
    ) -> Self {
        let url = format!("http://127.0.0.1:{port}/");
        Shared {
            token,
            port,
            check: HostCheck::new(Exposure::Loopback, port, &url),
            location,
            last_heartbeat: Mutex::new(Instant::now()),
            launch_file,
            launch_removed: AtomicBool::new(false),
            report,
            report_cache: Mutex::new(None),
        }
    }

    fn since_heartbeat(&self) -> Duration {
        self.last_heartbeat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed()
    }

    fn remove_launch_file(&self) {
        if !self.launch_removed.swap(true, Ordering::SeqCst) {
            let _ = std::fs::remove_file(&self.launch_file);
        }
    }
}

/// 256 random bits as 64 hex digits.
pub fn new_token() -> Result<String, String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "cannot read random bytes for the UI's token".to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether `a` and `b` are equal, in time independent of where they differ.
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

/// The launch file of this process in `run_dir`.
pub fn launch_file(run_dir: &Path) -> PathBuf {
    run_dir.join(format!("{LAUNCH_PREFIX}{}.html", std::process::id()))
}

/// The launch file's text: a redirect to the page with the token in the fragment.
fn launch_text(url: &str) -> String {
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><meta name=\"referrer\" \
         content=\"no-referrer\"><title>graphersal-memory</title>\n<meta http-equiv=\"refresh\" \
         content=\"0;url={url}\"></head>\n<body><script>location.replace({url:?});</script>\
         <p>Opening <a href=\"{url}\">your memory</a>...</p></body></html>\n"
    )
}

/// Writes the launch file (0600 on Unix).
fn write_launch_file(path: &Path, url: &str) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    file.write_all(launch_text(url).as_bytes())
        .and_then(|()| file.flush())
        .map_err(|err| format!("cannot write {}: {err}", path.display()))
}

/// Removes launch files of earlier runs older than `timeout` (a running `ui` removes its own
/// at the first heartbeat, or exits after `timeout` without one).
fn sweep_launch_files(run_dir: &Path, timeout: Duration) {
    let Ok(entries) = std::fs::read_dir(run_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(LAUNCH_PREFIX) || !name.ends_with(".html") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|age| age > timeout);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Opens the browser on the launch file (its path is the only argument).
fn open_browser(file: &Path) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    } else {
        std::process::Command::new("xdg-open")
    };
    command
        .arg(file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|err| format!("cannot open the browser: {err}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Keeps one connection to the daemon open (it counts as activity, so the daemon does not exit
/// idle while `ui` runs), reconnecting when the daemon restarted, killed or crashed (starting a
/// new one, unless a daemon was stopped on purpose: `crate::shim::may_start`). The first
/// connection is made here (an error is returned); later ones on the thread.
fn keep_daemon(location: &DaemonLocation) -> Result<(), String> {
    let options = crate::shim::ShimOptions {
        location: location.clone(),
        profile: crate::cli::Profile::Use,
        project: String::new(),
        harness: "graphersal-memory-ui".to_string(),
        start_context: crate::protocol::StartContextMode::None,
    };
    let first = crate::shim::open(&options)?;
    std::thread::Builder::new()
        .name("daemon".into())
        .spawn(move || {
            let mut connection = Some(first);
            loop {
                if let Some((stream, mut reader)) = connection.take() {
                    // Nothing is sent, so nothing comes back until the daemon closes it.
                    let _ = std::io::copy(&mut reader, &mut std::io::sink());
                    drop(stream);
                }
                std::thread::sleep(Duration::from_secs(1));
                let start = crate::shim::may_start(&options.location.run_dir);
                connection = crate::shim::open_with(&options, start)
                    .ok()
                    .map(|opened| (opened.stream, opened.reader));
            }
        })
        .map_err(|err| format!("cannot start the daemon connection: {err}"))?;
    Ok(())
}

/// Runs `ui` until no page sent a heartbeat for the timeout (or Ctrl+C ends the process).
/// Prints the address with the token to `out`.
pub fn run(options: UiOptions, out: &mut dyn Write) -> Result<u8, String> {
    let run_dir = options.location.run_dir.clone();
    crate::ipc::create_run_dir(&run_dir)?;
    sweep_launch_files(&run_dir, options.heartbeat_timeout.max(HEARTBEAT_TIMEOUT));
    keep_daemon(&options.location)?;
    let server = tiny_http::Server::http(("127.0.0.1", options.port))
        .map_err(|err| format!("cannot listen on 127.0.0.1:{}: {err}", options.port))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|address| address.port())
        .ok_or("the UI server has no TCP port")?;
    let token = new_token()?;
    let url = format!("http://127.0.0.1:{port}/#token={token}");
    let file = launch_file(&run_dir);
    write_launch_file(&file, &url)?;
    let shared = Arc::new(Shared::new(
        token,
        port,
        options.location.clone(),
        file.clone(),
        options.report.clone(),
    ));
    let _ = writeln!(out, "graphersal-memory ui: your memory at {url}");
    let _ = writeln!(
        out,
        "Open that address in your browser if no tab opened. Stop with Ctrl+C; the UI also stops \
         {} minutes after its last browser tab closed.",
        options.heartbeat_timeout.as_secs().div_ceil(60)
    );
    let _ = out.flush();
    if options.open_browser
        && let Err(err) = open_browser(&file)
    {
        let _ = writeln!(out, "{err}; open the address above by hand.");
        let _ = out.flush();
    }
    loop {
        if shared.since_heartbeat() > options.heartbeat_timeout {
            break;
        }
        match server.recv_timeout(Duration::from_millis(500)) {
            Ok(Some(request)) => {
                let shared = shared.clone();
                let spawned = std::thread::Builder::new()
                    .name("request".into())
                    .spawn(move || serve(&shared, request));
                if let Err(err) = spawned {
                    let _ = writeln!(out, "cannot answer a request: {err}");
                }
            }
            Ok(None) => {}
            Err(err) => {
                let _ = writeln!(out, "the UI server failed: {err}");
                break;
            }
        }
    }
    shared.remove_launch_file();
    let _ = writeln!(
        out,
        "graphersal-memory ui: stopped (no browser tab for {} minutes).",
        options.heartbeat_timeout.as_secs().div_ceil(60)
    );
    Ok(0)
}

/// Answers one request of the HTTP server.
fn serve(shared: &Shared, mut request: tiny_http::Request) {
    let method = request.method().to_string();
    let url = request.url().to_string();
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|header| (header.field.to_string(), header.value.to_string()))
        .collect();
    let length = request.body_length();
    let reply = answer(shared, &method, &url, &headers, length, request.as_reader());
    let mut response = tiny_http::Response::from_data(reply.body).with_status_code(reply.status);
    let all = common_headers()
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .chain(reply.headers);
    for (name, value) in all {
        if name.to_ascii_lowercase().starts_with("access-control-") {
            continue;
        }
        if let Ok(header) = tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response.add_header(header);
        }
    }
    let _ = request.respond(response);
}

/// The value of header `name` (any case).
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// The answer to one request: the Host/Origin check first, then static files, the token for
/// `/api/`, `/api/ui/*` here and the rest relayed.
pub fn answer(
    shared: &Shared,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    length: Option<usize>,
    body: &mut dyn Read,
) -> Reply {
    let mut checked = graphersal_serve::Request::new(
        graphersal_serve::Method::parse(method),
        url,
        std::io::empty(),
    );
    for (name, value) in headers {
        checked = checked.with_header(name, value);
    }
    if let Err(err) = shared.check.check(&checked) {
        return Reply::error(err.status(), err.message());
    }
    let path = url.split(['?', '#']).next().unwrap_or(url);
    if !path.starts_with("/api/") {
        return static_file(method, path);
    }
    let authorized = header(headers, "authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|given| same_bytes(given.trim().as_bytes(), shared.token.as_bytes()));
    if !authorized {
        return Reply::error(
            401,
            "this request needs the UI's token: open the address graphersal-memory ui printed \
             (or start it again)",
        );
    }
    // A request without a declared length or chunked body has none (reading it would wait for
    // the browser to close a kept-alive connection).
    let chunked = header(headers, "transfer-encoding").is_some_and(|v| v.contains("chunked"));
    let mut empty = std::io::empty();
    let body: &mut dyn Read = if length.is_some_and(|n| n > 0) || chunked {
        body
    } else {
        &mut empty
    };
    let body = match read_body(length, body) {
        Ok(body) => body,
        Err(reply) => return reply,
    };
    if let Some(rest) = path.strip_prefix("/api/ui/") {
        return ui_api(shared, method, rest, url, headers, &body);
    }
    match crate::relay::send(&shared.location, shared.port, method, url, headers, &body) {
        Ok(answer) => Reply {
            status: answer.status,
            headers: answer
                .headers
                .into_iter()
                .filter(|(name, _)| {
                    let name = name.to_ascii_lowercase();
                    !name.starts_with("access-control-") && name != "content-length"
                })
                .collect(),
            body: answer.body,
        },
        Err(err) => Reply::error(502, &err),
    }
}

/// The request body, at most the relay's limit.
fn read_body(length: Option<usize>, body: &mut dyn Read) -> Result<Vec<u8>, Reply> {
    let limit = crate::relay::MAX_BODY_BYTES;
    if length.is_some_and(|n| n > limit) {
        return Err(Reply::error(
            413,
            &format!("the request body is larger than {limit} bytes"),
        ));
    }
    let mut bytes = Vec::new();
    body.take(u64::try_from(limit).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| Reply::error(400, &format!("cannot read the request body: {err}")))?;
    if bytes.len() > limit {
        return Err(Reply::error(
            413,
            &format!("the request body is larger than {limit} bytes"),
        ));
    }
    Ok(bytes)
}

/// The tag that loads the profile's boot script and style sheet into the playground's page.
const BOOT_TAGS: &str = "\n  <script src=\"/memory-ui/boot.js\"></script>\n  <link rel=\"stylesheet\" \
                         href=\"/memory-ui/memory-ui.css\">";

/// A static file: the profile's own first, then the embedded playground.
fn static_file(method: &str, path: &str) -> Reply {
    if method != "GET" && method != "HEAD" {
        return Reply::error(
            405,
            &format!("{method} {path}: static files answer GET only"),
        );
    }
    if let Some(name) = path.strip_prefix(crate::ui_assets::FILES_PREFIX) {
        return match crate::ui_assets::file(name) {
            Some((bytes, kind)) => Reply {
                status: 200,
                headers: vec![("Content-Type".into(), kind.into())],
                body: bytes.to_vec(),
            },
            None => Reply::error(404, &format!("no file {path}")),
        };
    }
    let relative = percent_decode(path.trim_start_matches('/'));
    let Some((bytes, name)) = graphersal_serve::Ui::embedded().read(&relative) else {
        return Reply::error(404, &format!("no file {path}"));
    };
    let bytes = if name == "index.html" {
        let text = String::from_utf8_lossy(&bytes);
        match text.find("<head>") {
            Some(at) => {
                let at = at + "<head>".len();
                format!("{}{BOOT_TAGS}{}", &text[..at], &text[at..]).into_bytes()
            }
            None => bytes,
        }
    } else {
        bytes
    };
    Reply {
        status: 200,
        headers: vec![(
            "Content-Type".into(),
            crate::ui_assets::content_type(&name).into(),
        )],
        body: bytes,
    }
}

/// `%XX` sequences decoded (`+` stays); an invalid sequence stays as it is.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            out.push((high * 16 + low) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The query parameter `name` of `url`, decoded.
fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (percent_decode(key) == name).then(|| percent_decode(&value.replace('+', " ")))
    })
}

/// `/api/ui/<rest>`: what `ui` answers itself.
fn ui_api(
    shared: &Shared,
    method: &str,
    rest: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Reply {
    let data_dir = &shared.location.data_dir;
    let run_dir = &shared.location.run_dir;
    match (method, rest) {
        ("POST", "heartbeat") => {
            *shared
                .last_heartbeat
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
            // The page is open: the browser no longer needs the launch file.
            shared.remove_launch_file();
            Reply::json(200, &json!({"ok": true}))
        }
        ("GET", "status") => {
            let daemon = crate::shim::control(run_dir, "status").ok().flatten();
            let sessions = crate::shim::control(run_dir, crate::sessions::CONTROL_STATUS)
                .ok()
                .flatten();
            let report = {
                let mut cache = shared
                    .report_cache
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match cache.as_ref() {
                    Some((at, report)) if at.elapsed() < REPORT_FOR => report.clone(),
                    _ => {
                        let report = (shared.report)();
                        *cache = Some((Instant::now(), report.clone()));
                        report
                    }
                }
            };
            Reply::json(
                200,
                &json!({"version": crate::protocol::VERSION, "daemon": daemon,
                        "sessions": sessions, "status": report["status"],
                        "doctor": report["doctor"]}),
            )
        }
        ("GET", "projects") => match crate::projects::Projects::load(data_dir) {
            Ok(projects) => {
                let list: Vec<Value> = projects
                    .list()
                    .into_iter()
                    .map(|project| {
                        json!({"id": project.id, "name": project_name(&project),
                               "paths": project.paths, "remotes": project.remotes})
                    })
                    .collect();
                Reply::json(200, &json!({"projects": list}))
            }
            Err(err) => Reply::error(500, &err),
        },
        ("GET", "start-context") => {
            let project = query_param(url, "project").unwrap_or_default();
            let line = json!({"protocol": crate::protocol::PROTOCOL, "control": "start_context",
                              "project_id": project, "entries": true});
            // Like the relay: a daemon that went away is started again.
            let answer = if crate::shim::may_start(run_dir) {
                crate::shim::request(&shared.location, &line).map(Some)
            } else {
                crate::shim::request_running(run_dir, &line)
            };
            match answer {
                Ok(Some(answer)) if answer["ok"] == true => Reply::json(200, &answer),
                Ok(Some(answer)) => Reply::error(
                    502,
                    answer["error"]["message"]
                        .as_str()
                        .unwrap_or("the daemon refused the start context"),
                ),
                Ok(None) => Reply::error(
                    503,
                    "the daemon was stopped on purpose and is not running; start the UI again",
                ),
                Err(err) => Reply::error(502, &err),
            }
        }
        ("GET", "settings") => match crate::config::Config::load(data_dir) {
            Ok(config) => Reply::json(200, &settings_json(&config)),
            Err(err) => Reply::error(500, &err),
        },
        ("POST", "settings") => {
            let json_type = header(headers, "content-type")
                .is_some_and(|value| value.starts_with("application/json"));
            if !json_type {
                return Reply::error(415, "the settings are sent as application/json");
            }
            match write_settings(data_dir, body) {
                Ok(config) => Reply::json(200, &settings_json(&config)),
                Err(err) => Reply::error(400, &err),
            }
        }
        ("GET", "update") => {
            let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
            let configured = crate::config::Config::load(data_dir)
                .ok()
                .and_then(|config| config.update_check);
            let enabled = crate::update::check_enabled(&env, configured);
            let releases = crate::update::releases_url(&env);
            let check = crate::update::check_cached(
                data_dir,
                enabled,
                std::time::SystemTime::now(),
                &|| crate::update::latest(&releases),
            );
            let method = match (std::env::current_exe(), crate::store::home_dir(&env)) {
                (Ok(exe), Ok(home)) => crate::update::method_of(&exe, &home, &env, cfg!(windows)),
                _ => crate::update::Method::Other(PathBuf::from(".")),
            };
            Reply::json(
                200,
                &crate::update::check_json(&check, &method, cfg!(windows)),
            )
        }
        (_, "heartbeat" | "status" | "projects" | "start-context" | "settings" | "update") => {
            Reply::error(
                405,
                &format!("{method} /api/ui/{rest} is not offered (GET reads, POST writes)"),
            )
        }
        _ => Reply::error(404, &format!("unknown endpoint /api/ui/{rest}")),
    }
}

/// A project's short name: the last part of its newest path, else of its id.
fn project_name(project: &crate::projects::Project) -> String {
    let from = project.paths.first().unwrap_or(&project.id);
    from.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(from)
        .to_string()
}

/// The settings the UI shows and sets.
fn settings_json(config: &crate::config::Config) -> Value {
    json!({"start_context_chars": config.start_context_chars(),
           "default_start_context_chars": crate::start_context::DEFAULT_CHARS,
           "max_start_context_chars": MAX_START_CONTEXT_CHARS})
}

/// Sets `memory.start_context_chars` from a body `{"start_context_chars": n}` in the settings
/// file (every other byte kept, `crate::config::with_values`); answers the new settings.
fn write_settings(data_dir: &Path, body: &[u8]) -> Result<crate::config::Config, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|err| format!("the body is not JSON: {err}"))?;
    let object = value.as_object().ok_or("the body must be a JSON object")?;
    if let Some(key) = object.keys().find(|key| *key != "start_context_chars") {
        return Err(format!("the UI sets only start_context_chars, not {key}"));
    }
    let chars = object
        .get("start_context_chars")
        .and_then(Value::as_u64)
        .filter(|n| *n <= MAX_START_CONTEXT_CHARS)
        .ok_or_else(|| {
            format!(
                "start_context_chars must be a whole number from 0 to {MAX_START_CONTEXT_CHARS}"
            )
        })?;
    let path = crate::config::path(data_dir);
    let current = crate::fsutil::read_optional(&path)?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let text = crate::config::with_values(
        current.as_deref(),
        &[("memory", "start_context_chars", json!(chars))],
    )?;
    std::fs::create_dir_all(data_dir)
        .map_err(|err| format!("cannot create {}: {err}", data_dir.display()))?;
    let before = current.clone();
    crate::fsutil::write_atomic_checked(&path, text.as_bytes(), &mut |target| {
        let now = std::fs::read(target)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        now == before
    })?;
    crate::config::Config::load(data_dir)
}

/// `text` without terminal colour sequences (`ESC [ ... m`).
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(dir: &Path) -> Shared {
        Shared::new(
            "ab".repeat(32),
            4100,
            DaemonLocation {
                data_dir: dir.join("data"),
                run_dir: dir.join("run"),
                exe: PathBuf::from("/nonexistent"),
                start_timeout: Duration::from_secs(1),
            },
            dir.join("run").join("ui-1.html"),
            Arc::new(|| json!({"status": ["daemon: ok"], "doctor": []})),
        )
    }

    fn get(shared: &Shared, url: &str, headers: &[(&str, &str)]) -> Reply {
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        answer(shared, "GET", url, &headers, None, &mut std::io::empty())
    }

    #[test]
    fn tokens_are_256_random_bits_and_compared_whole() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(same_bytes(a.as_bytes(), a.as_bytes()));
        assert!(!same_bytes(a.as_bytes(), b.as_bytes()));
        assert!(!same_bytes(a.as_bytes(), &a.as_bytes()[..63]));
    }

    #[test]
    fn the_host_check_runs_before_the_token_and_static_files_need_none() {
        let dir = crate::mcp_tests::TempDir::new("ui-unit");
        let shared = shared(dir.path());
        let host = ("Host", "127.0.0.1:4100");
        let wrong_host = get(&shared, "/", &[("Host", "evil.example:4100")]);
        assert_eq!(wrong_host.status, 403);
        let foreign = get(
            &shared,
            "/api/ui/settings",
            &[host, ("Origin", "http://evil.example")],
        );
        assert_eq!(foreign.status, 403);
        let no_token = get(&shared, "/api/ui/settings", &[host]);
        assert_eq!(no_token.status, 401);
        let wrong = get(
            &shared,
            "/api/ui/settings",
            &[
                host,
                ("Authorization", &format!("Bearer {}", "cd".repeat(32))),
            ],
        );
        assert_eq!(wrong.status, 401);
        let bearer = format!("Bearer {}", "ab".repeat(32));
        let settings = get(
            &shared,
            "/api/ui/settings",
            &[host, ("Authorization", &bearer)],
        );
        assert_eq!(
            settings.status,
            200,
            "{:?}",
            String::from_utf8_lossy(&settings.body)
        );
        let page = get(&shared, "/", &[host]);
        assert_eq!(page.status, 200);
        let text = String::from_utf8_lossy(&page.body);
        assert!(text.contains("/memory-ui/boot.js"), "{text}");
        let module = get(&shared, "/memory-ui/model.js", &[host]);
        assert_eq!(module.status, 200);
        assert_eq!(get(&shared, "/memory-ui/nope.js", &[host]).status, 404);
        assert_eq!(get(&shared, "/../Cargo.toml", &[host]).status, 404);
        let post = answer(
            &shared,
            "POST",
            "/index.html",
            &[(host.0.to_string(), host.1.to_string())],
            None,
            &mut std::io::empty(),
        );
        assert_eq!(post.status, 405);
    }

    #[test]
    fn the_update_check_answers_from_its_kept_state_or_not_at_all() {
        let dir = crate::mcp_tests::TempDir::new("ui-update");
        let shared = shared(dir.path());
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let headers = [
            ("Host", "127.0.0.1:4100"),
            ("Authorization", &*format!("Bearer {}", "ab".repeat(32))),
        ];
        let body = |reply: Reply| -> Value {
            assert_eq!(
                reply.status,
                200,
                "{}",
                String::from_utf8_lossy(&reply.body)
            );
            serde_json::from_slice(&reply.body).unwrap()
        };
        // A check kept a minute ago: answered from it (no request), a newer release shown with
        // the commands to copy.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        std::fs::write(
            data.join(crate::update::STATE_FILE),
            json!({"checked_at": now - 60, "latest": "99.0.0", "error": null}).to_string(),
        )
        .unwrap();
        let update = body(get(&shared, "/api/ui/update", &headers));
        assert_eq!(update["enabled"], true, "{update}");
        assert_eq!(update["latest"], "99.0.0");
        assert_eq!(update["newer"], true);
        assert_eq!(update["current"], crate::update::CURRENT);
        assert_eq!(update["upgrade"], "graphersal-memory upgrade");
        assert!(
            update["manual"].as_array().is_some_and(|m| m.len() == 2),
            "{update}"
        );
        assert!(
            update["release_notes"]
                .as_str()
                .unwrap()
                .ends_with("/tag/v99.0.0")
        );
        // updates.check false: nothing asked, nothing shown.
        std::fs::write(
            data.join(crate::config::CONFIG_FILE),
            r#"{"version": 1, "updates": {"check": false}}"#,
        )
        .unwrap();
        let off = body(get(&shared, "/api/ui/update", &headers));
        assert_eq!(
            (off["enabled"].clone(), off["newer"].clone()),
            (json!(false), json!(false))
        );
        assert!(off["latest"].is_null(), "{off}");
    }

    #[test]
    fn settings_change_only_the_budget_in_place() {
        let dir = crate::mcp_tests::TempDir::new("ui-settings");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            crate::config::path(&data),
            "{\n  \"version\": 1,\n  \"daemon\": {\"idle\": \"3h\"},\n  \"memory\": {\"language\": \"any\"}\n}\n",
        )
        .unwrap();
        let config = write_settings(&data, br#"{"start_context_chars": 4000}"#).unwrap();
        assert_eq!(config.start_context_chars(), 4000);
        let text = std::fs::read_to_string(crate::config::path(&data)).unwrap();
        assert!(text.contains("\"idle\": \"3h\""), "{text}");
        assert!(text.contains("\"language\": \"any\""), "{text}");
        assert!(write_settings(&data, br#"{"language": "english"}"#).is_err());
        assert!(write_settings(&data, br#"{"start_context_chars": -1}"#).is_err());
        assert!(write_settings(&data, b"nope").is_err());
    }

    #[test]
    fn helpers_decode_and_strip() {
        assert_eq!(percent_decode("%2Fw%2Fapp"), "/w/app");
        assert_eq!(percent_decode("a%zz%4"), "a%zz%4");
        assert_eq!(
            query_param("/api/ui/start-context?x=1&project=%2Fw%2Fapp", "project").as_deref(),
            Some("/w/app")
        );
        assert_eq!(query_param("/api/ui/start-context", "project"), None);
        assert_eq!(plain("\u{1b}[1;33mPROBLEM\u{1b}[0m x"), "PROBLEM x");
        assert!(launch_text("http://127.0.0.1:1/#token=t").contains("location.replace"));
    }
}
