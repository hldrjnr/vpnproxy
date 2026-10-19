#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State,
};

/// Nenhum terminal abre/fecha na tela: tudo roda em janela oculta.
const NO_WINDOW: u32 = 0x08000000;
/// Nenhuma etapa pode pendurar a app além disto.
const STEP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize)]
struct StepOut {
    ok: bool,
    out: String,
    ms: u64,
    timed_out: bool,
}

/// Executa um programa diretamente (sem cmd, sem shell) com tempo-limite:
/// se estourar, o processo é morto e a chamada retorna — nunca trava.
fn run_timeout(prog: &str, args: &[&str], timeout: Duration) -> StepOut {
    use std::io::Read;
    let start = Instant::now();
    let fail = |out: String, timed_out: bool| StepOut {
        ok: false,
        out,
        ms: start.elapsed().as_millis() as u64,
        timed_out,
    };
    let mut child = match Command::new(prog)
        .args(args)
        .creation_flags(NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return fail(e.to_string(), false),
    };
    // Leitor do stdout em thread separada (saídas pequenas; nunca bloqueia o controle).
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut o) = stdout {
            let _ = o.read_to_end(&mut buf);
        }
        buf
    });
    let mut timed_out = false;
    let status_ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st.success()),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return fail(e.to_string(), false),
        }
    };
    let buf = reader.join().unwrap_or_default();
    let out = String::from_utf8_lossy(&buf).trim().to_string();
    StepOut {
        ok: status_ok.unwrap_or(false) && !timed_out,
        out,
        ms: start.elapsed().as_millis() as u64,
        timed_out,
    }
}

/// Atalho com o tempo-limite padrão.
fn run(prog: &str, args: &[&str]) -> StepOut {
    run_timeout(prog, args, STEP_TIMEOUT)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- admin

fn is_elevated() -> bool {
    run("net", &["session"]).ok
}

/// A app sempre roda como admin (igual aos .bat): se não estiver elevada,
/// relança a si mesma via UAC e encerra esta instância — sem terminal.
fn ensure_admin() {
    if is_elevated() {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new("powershell")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &format!("Start-Process -FilePath \"{}\" -Verb RunAs", exe.display()),
            ])
            .creation_flags(NO_WINDOW)
            .spawn();
    }
    std::process::exit(0);
}

#[tauri::command]
fn is_admin() -> bool {
    is_elevated()
}

// ---------------------------------------------------------------- proxy

const INET_KEY: &str = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
const ENV_KEY: &str = "HKCU\\Environment";
const DEFAULT_BYPASS: &str = "localhost;127.0.0.1;*.local";
/// Hosts internos pelos quais a própria app fala com o backend (IPC).
/// Têm que passar direto SEMPRE, senão ligar o proxy quebra/congela a app.
const INTERNAL_HOSTS: [&str; 3] = ["ipc.localhost", "tauri.localhost", "asset.localhost"];

/// Última configuração usada (para o menu da bandeja ligar sem abrir a janela).
struct LastCfg(Mutex<(String, u16, String)>);

/// Garante os hosts internos na lista de exceções (sem duplicar).
fn with_internal_bypass(bypass: &str) -> String {
    let mut parts: Vec<String> = bypass
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for h in INTERNAL_HOSTS {
        if !parts.iter().any(|p| p.eq_ignore_ascii_case(h)) {
            parts.push(h.to_string());
        }
    }
    parts.join(";")
}

#[derive(Debug, Clone, Serialize)]
struct StepInfo {
    name: String,
    ms: u64,
    ok: bool,
    timed_out: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ToggleResult {
    ok: bool,
    winhttp_ok: bool,
    steps: Vec<StepInfo>,
}

/// Executa uma etapa com timeout e registra o resultado (para diagnóstico).
fn step(steps: &mut Vec<StepInfo>, name: &str, prog: &str, args: &[&str]) -> bool {
    let r = run(prog, args);
    let ok = r.ok && !r.timed_out;
    steps.push(StepInfo {
        name: name.into(),
        ms: r.ms,
        ok,
        timed_out: r.timed_out,
    });
    ok
}

fn apply_enable(host: &str, port: u16, bypass: &str) -> Result<ToggleResult, String> {
    let host = host.trim().to_string();
    if host.len() < 3 || host.len() > 255 {
        return Err("IP/host inválido.".into());
    }
    if port == 0 {
        return Err("Porta inválida (1–65535).".into());
    }
    let bypass = if bypass.trim().is_empty() {
        DEFAULT_BYPASS.to_string()
    } else {
        bypass.trim().to_string()
    };
    // Crítico: o IPC da app (ipc.localhost etc.) nunca pode passar pelo proxy.
    let bypass = with_internal_bypass(&bypass);
    let server = format!("{host}:{port}");
    let url = format!("http://{server}");
    let no_proxy = bypass.replace(';', ",");
    let mut steps = Vec::new();

    // 1 - WinHTTP (serviços do Windows)
    let proxy_param = format!("proxy-server=http={server};https={server}");
    let bypass_param = format!("bypass-list={bypass}");
    let winhttp_ok = step(
        &mut steps,
        "winhttp",
        "netsh",
        &["winhttp", "set", "proxy", &proxy_param, &bypass_param],
    );

    // 2 - WinINET (navegadores e apps)
    let mut ok = true;
    ok &= step(
        &mut steps,
        "wininet-enable",
        "reg",
        &["add", INET_KEY, "/v", "ProxyEnable", "/t", "REG_DWORD", "/d", "1", "/f"],
    );
    ok &= step(
        &mut steps,
        "wininet-server",
        "reg",
        &["add", INET_KEY, "/v", "ProxyServer", "/t", "REG_SZ", "/d", &server, "/f"],
    );
    ok &= step(
        &mut steps,
        "wininet-bypass",
        "reg",
        &["add", INET_KEY, "/v", "ProxyOverride", "/t", "REG_SZ", "/d", &bypass, "/f"],
    );

    // 3 - variáveis persistentes (reg direto: setx faz broadcast e pode pendurar)
    for (k, v) in [
        ("HTTP_PROXY", url.as_str()),
        ("HTTPS_PROXY", url.as_str()),
        ("ALL_PROXY", url.as_str()),
        ("NO_PROXY", no_proxy.as_str()),
    ] {
        ok &= step(&mut steps, &format!("env-{k}"), "reg", &["add", ENV_KEY, "/v", k, "/t", "REG_SZ", "/d", v, "/f"]);
    }

    // 4 - DNS
    step(&mut steps, "flushdns", "ipconfig", &["/flushdns"]);

    Ok(ToggleResult { ok, winhttp_ok, steps })
}

fn apply_disable() -> Result<ToggleResult, String> {
    let mut steps = Vec::new();
    let winhttp_ok = step(&mut steps, "winhttp", "netsh", &["winhttp", "reset", "proxy"]);

    let ok = step(
        &mut steps,
        "wininet-disable",
        "reg",
        &["add", INET_KEY, "/v", "ProxyEnable", "/t", "REG_DWORD", "/d", "0", "/f"],
    );

    for v in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
        // falhar aqui só quer dizer que a variável nem existia
        step(&mut steps, &format!("env-del-{v}"), "reg", &["delete", ENV_KEY, "/v", v, "/f"]);
    }

    step(&mut steps, "flushdns", "ipconfig", &["/flushdns"]);

    Ok(ToggleResult { ok, winhttp_ok, steps })
}

#[tauri::command]
fn enable_proxy(cfg: State<LastCfg>, host: String, port: u16, bypass: String) -> Result<ToggleResult, String> {
    let r = apply_enable(&host, port, &bypass)?;
    if let Ok(mut g) = cfg.0.lock() {
        *g = (host.trim().to_string(), port, bypass.trim().to_string());
    }
    Ok(r)
}

#[tauri::command]
fn disable_proxy() -> Result<ToggleResult, String> {
    apply_disable()
}

fn reg_value(key: &str, name: &str) -> String {
    let r = run("reg", &["query", key, "/v", name]);
    for line in r.out.lines() {
        let line = line.trim();
        if line.starts_with(name) {
            // formato: Nome  TIPO  dados (dados podem conter espaços)
            let mut parts = line.split_whitespace();
            parts.next(); // nome
            parts.next(); // tipo
            return parts.collect::<Vec<_>>().join(" ");
        }
    }
    String::new()
}

#[derive(Serialize)]
struct ProxyStatus {
    enabled: bool,
    server: String,
    bypass: String,
    winhttp: String,
}

#[tauri::command]
fn get_status() -> ProxyStatus {
    let enabled = reg_value(INET_KEY, "ProxyEnable") == "0x1";
    let server = reg_value(INET_KEY, "ProxyServer");
    let bypass = reg_value(INET_KEY, "ProxyOverride");
    let winhttp = run("netsh", &["winhttp", "show", "proxy"]).out;
    ProxyStatus {
        enabled,
        server,
        bypass,
        winhttp,
    }
}

// ---------------------------------------------------------------- rede

static PREV: Mutex<Option<(u64, u64, u64)>> = Mutex::new(None);

/// Amostra nativa via GetIfTable2: uma syscall, sem spawn de processo.
/// É isto que torna o monitor de 1s barato (microssegundos, sem .NET, sem CIM).
/// Mede TODO o tráfego da placa — incluindo o que passa pelo proxy.
fn native_sample() -> Result<(u64, u64, usize), String> {
    use windows::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK,
    };
    unsafe {
        let mut table = std::ptr::null_mut();
        let ret = GetIfTable2(&mut table);
        if ret.0 != 0 || table.is_null() {
            return Err(format!("GetIfTable2 falhou: {}", ret.0));
        }
        let num = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), num);
        let mut rx = 0u64;
        let mut tx = 0u64;
        let mut n = 0usize;
        for row in rows {
            if row.Type == IF_TYPE_SOFTWARE_LOOPBACK {
                continue;
            }
            rx += row.InOctets;
            tx += row.OutOctets;
            n += 1;
        }
        FreeMibTable(table.cast());
        Ok((rx, tx, n))
    }
}

/// Fallback antigo (PowerShell): só se a syscall nativa falhar.
fn ps_sample() -> Result<(u64, u64, usize), String> {
    let ps = "Get-NetAdapterStatistics | Where-Object { $_.Name -notmatch 'Loopback' } | Select-Object ReceivedBytes,SentBytes | ConvertTo-Json -Compress";
    let r = run("powershell", &["-NoProfile", "-Command", ps]);
    if !r.ok {
        return Err("Falha ao ler os adaptadores de rede.".into());
    }
    let v: serde_json::Value = serde_json::from_str(&r.out).map_err(|e| e.to_string())?;
    let items: Vec<&serde_json::Value> = match &v {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(_) => vec![&v],
        _ => vec![],
    };
    let (mut rx, mut tx) = (0u64, 0u64);
    for a in &items {
        rx += a
            .get("ReceivedBytes")
            .and_then(|x| x.as_u64())
            .unwrap_or(0);
        tx += a.get("SentBytes").and_then(|x| x.as_u64()).unwrap_or(0);
    }
    Ok((rx, tx, items.len()))
}

#[derive(Serialize)]
struct NetSample {
    rx: u64,
    tx: u64,
    rx_rate: f64,
    tx_rate: f64,
    adapters: usize,
}

#[tauri::command]
fn net_sample() -> Result<NetSample, String> {
    let (rx, tx, adapters) = match native_sample() {
        Ok(v) => v,
        Err(_) => ps_sample()?,
    };

    let now = now_ms();
    let (rx_rate, tx_rate) = {
        let mut prev = PREV.lock().map_err(|e| e.to_string())?;
        let rates = match *prev {
            Some((prx, ptx, t)) => {
                let dt = ((now.saturating_sub(t)) as f64 / 1000.0).max(0.2);
                (
                    (rx.saturating_sub(prx) as f64 / dt).max(0.0),
                    (tx.saturating_sub(ptx) as f64 / dt).max(0.0),
                )
            }
            None => (0.0, 0.0),
        };
        *prev = Some((rx, tx, now));
        rates
    };

    Ok(NetSample {
        rx,
        tx,
        rx_rate,
        tx_rate,
        adapters,
    })
}

// ---------------------------------------------------------------- bandeja

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let png = include_bytes!("../icons/128x128.png");
    let rgba = image::load_from_memory(png)
        .expect("icone embutido inválido")
        .to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let icon = tauri::image::Image::new_owned(rgba.into_raw(), w, h);

    let show = MenuItem::with_id(app, "show", "Mostrar", true, None::<&str>)?;
    let on = MenuItem::with_id(app, "on", "Ligar proxy", true, None::<&str>)?;
    let off = MenuItem::with_id(app, "off", "Desligar proxy", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&show, &on, &off, &PredefinedMenuItem::separator(app)?, &quit],
    )?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Proxy Toggle")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if matches!(event, TrayIconEvent::Click { .. }) {
                show_main(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "on" => {
                let h = app.clone();
                std::thread::spawn(move || {
                    let cfg: (String, u16, String) = h
                        .state::<LastCfg>()
                        .0
                        .lock()
                        .map(|g| g.clone())
                        .unwrap_or(("10.71.145.245".into(), 8080, String::new()));
                    let _ = apply_enable(&cfg.0, cfg.1, &cfg.2);
                    let _ = h.emit("proxy-changed", ());
                });
            }
            "off" => {
                let h = app.clone();
                std::thread::spawn(move || {
                    let _ = apply_disable();
                    let _ = h.emit("proxy-changed", ());
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

// ---------------------------------------------------------------- main

fn main() {
    ensure_admin();
    tauri::Builder::default()
        .manage(LastCfg(Mutex::new((
            "10.71.145.245".into(),
            8080u16,
            String::new(),
        ))))
        .setup(|app| {
            build_tray(app.handle())?;
            // Minimizar (e fechar) oculta para a bandeja em vez de encerrar.
            let h = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(300));
                match h.get_webview_window("main") {
                    Some(w) => {
                        if w.is_minimized().unwrap_or(false) {
                            let _ = w.hide();
                        }
                    }
                    None => break,
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            is_admin,
            enable_proxy,
            disable_proxy,
            get_status,
            net_sample
        ])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o Proxy Toggle");
}

// ---------------------------------------------------------------- testes

#[cfg(test)]
mod tests {
    use super::*;

    const TKEY: &str = "HKCU\\Software\\ProxyToggleSelfTest";
    const ENV_VARS: [&str; 4] = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"];

    #[test]
    fn bypass_sempre_inclui_hosts_internos() {
        let b = with_internal_bypass("localhost;127.0.0.1;*.local");
        for h in INTERNAL_HOSTS {
            assert!(b.contains(h), "faltando {h} em: {b}");
        }
        // sem duplicar se o usuário já incluiu
        let b2 = with_internal_bypass("ipc.localhost;foo.com");
        assert_eq!(b2.matches("ipc.localhost").count(), 1, "{b2}");
    }

    #[test]
    fn comandos_diretos_sem_shell() {
        let data = "localhost;127.0.0.1;*.local com espaco";
        let r = run("reg", &["add", TKEY, "/v", "Data", "/t", "REG_SZ", "/d", data, "/f"]);
        assert!(r.ok && !r.timed_out, "reg add falhou: {r:?}");
        let q = run("reg", &["query", TKEY, "/v", "Data"]);
        assert!(q.out.contains(data), "valor gravado diverge: {}", q.out);
        let _ = run("reg", &["delete", TKEY, "/f"]);
    }

    #[test]
    fn status_lez_valores_limpos() {
        let s = get_status();
        assert!(
            !s.server.contains(['"', '\\']),
            "ProxyServer contaminado: {}",
            s.server
        );
    }

    #[test]
    fn amostra_nativa_rapida() {
        let t = Instant::now();
        let (rx, tx, n) = native_sample().expect("GetIfTable2 falhou");
        let _ = (rx, tx);
        assert!(n > 0, "nenhum adaptador");
        assert!(t.elapsed() < Duration::from_secs(2), "amostra lenta: {:?}", t.elapsed());
    }

    /// Restaura o estado real do proxy mesmo se o teste falhar no meio.
    struct Restore {
        enable: String,
        server: String,
        bypass: String,
        env: Vec<(String, String)>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            let dword = if self.enable == "0x1" { "1" } else { "0" };
            let _ = run("reg", &["add", INET_KEY, "/v", "ProxyEnable", "/t", "REG_DWORD", "/d", dword, "/f"]);
            if self.server.is_empty() {
                let _ = run("reg", &["delete", INET_KEY, "/v", "ProxyServer", "/f"]);
            } else {
                let _ = run("reg", &["add", INET_KEY, "/v", "ProxyServer", "/t", "REG_SZ", "/d", &self.server, "/f"]);
            }
            if self.bypass.is_empty() {
                let _ = run("reg", &["delete", INET_KEY, "/v", "ProxyOverride", "/f"]);
            } else {
                let _ = run("reg", &["add", INET_KEY, "/v", "ProxyOverride", "/t", "REG_SZ", "/d", &self.bypass, "/f"]);
            }
            for (k, v) in &self.env {
                if v.is_empty() {
                    let _ = run("reg", &["delete", ENV_KEY, "/v", k, "/f"]);
                } else {
                    let _ = run("reg", &["add", ENV_KEY, "/v", k, "/t", "REG_SZ", "/d", v, "/f"]);
                }
            }
        }
    }

    /// Executa o caminho REAL de ligar+desligar (com proxy fictício) e exige
    /// conclusão rápida: se alguma etapa pendurar, os timeouts estouram e o
    /// teste aponta exatamente qual.
    #[test]
    fn toggle_dummy_conclui_rapido() {
        let guard = Restore {
            enable: reg_value(INET_KEY, "ProxyEnable"),
            server: reg_value(INET_KEY, "ProxyServer"),
            bypass: reg_value(INET_KEY, "ProxyOverride"),
            env: ENV_VARS.iter().map(|k| (k.to_string(), reg_value(ENV_KEY, k))).collect(),
        };
        let t = Instant::now();
        let e = apply_enable("127.0.0.1", 59999, "localhost").expect("enable errou");
        assert!(e.ok, "enable falhou: {:?}", e.steps);
        assert!(!e.steps.iter().any(|s| s.timed_out), "etapa estourou: {:?}", e.steps);
        let stored = reg_value(INET_KEY, "ProxyOverride");
        assert!(stored.contains("ipc.localhost"), "bypass sem host interno: {stored}");
        let d = apply_disable().expect("disable errou");
        assert!(d.ok, "disable falhou: {:?}", d.steps);
        assert!(!d.steps.iter().any(|s| s.timed_out), "etapa estourou: {:?}", d.steps);
        let total = t.elapsed();
        assert!(total < Duration::from_secs(60), "toggle lento: {total:?}");
        drop(guard);
    }
}
