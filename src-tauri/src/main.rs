#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State,
};

/// (Windows) Nenhum terminal abre/fecha na tela: tudo roda em janela oculta.
#[cfg(windows)]
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
    let mut cmd = Command::new(prog);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(NO_WINDOW);
    let mut child = match cmd.spawn() {
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

// ---------------------------------------------------------------- proxy (comum)

/// Exceções padrão quando o campo vem vazio.
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

/// Quebra a lista de exceções em itens individais (';' ou ',').
fn bypass_entries(bypass: &str) -> Vec<String> {
    bypass
        .split(|c| c == ';' || c == ',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
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
    /// Etapa principal do sistema: WinHTTP (Windows) / gsettings (Linux).
    system_ok: bool,
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

/// Registra uma etapa executada em processo próprio (sem spawn), p.ex. escrever arquivo.
#[cfg(unix)]
fn inline_step(steps: &mut Vec<StepInfo>, name: &str, ok: bool) -> bool {
    steps.push(StepInfo {
        name: name.into(),
        ms: 0,
        ok,
        timed_out: false,
    });
    ok
}

#[derive(Serialize)]
struct ProxyStatus {
    enabled: bool,
    server: String,
    bypass: String,
    /// informativo: saída do WinHTTP (Windows) / modo do gsettings (Linux)
    winhttp: String,
}

// ---------------------------------------------------------------- plataforma: Windows

#[cfg(windows)]
mod platform {
    use super::*;

    // ---------------------------------------------------------------- admin

    pub fn is_elevated() -> bool {
        run("net", &["session"]).ok
    }

    /// A app sempre roda como admin (igual aos .bat): se não estiver elevada,
    /// relança a si mesma via UAC e encerra esta instância — sem terminal.
    pub fn ensure_admin() {
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

    // ---------------------------------------------------------------- proxy

    pub const INET_KEY: &str = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
    pub const ENV_KEY: &str = "HKCU\\Environment";

    pub fn apply_enable(host: &str, port: u16, bypass: &str) -> Result<ToggleResult, String> {
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
        let no_proxy = bypass_entries(&bypass).join(",");
        let mut steps = Vec::new();

        // 1 - WinHTTP (serviços do Windows)
        let proxy_param = format!("proxy-server=http={server};https={server}");
        let bypass_param = format!("bypass-list={bypass}");
        let system_ok = step(
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

        Ok(ToggleResult {
            ok: ok && system_ok,
            system_ok,
            steps,
        })
    }

    pub fn apply_disable() -> Result<ToggleResult, String> {
        let mut steps = Vec::new();
        let system_ok = step(&mut steps, "winhttp", "netsh", &["winhttp", "reset", "proxy"]);

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

        Ok(ToggleResult {
            ok: ok && system_ok,
            system_ok,
            steps,
        })
    }

    pub fn reg_value(key: &str, name: &str) -> String {
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

    pub fn status() -> ProxyStatus {
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

    /// Amostra nativa via GetIfTable2: uma syscall, sem spawn de processo.
    /// Mede TODO o tráfego da placa — incluindo o que passa pelo proxy.
    pub fn native_sample() -> Result<(u64, u64, usize), String> {
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
    pub fn ps_sample() -> Result<(u64, u64, usize), String> {
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

    // ---------------------------------------------------------------- testes

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{INTERNAL_HOSTS, with_internal_bypass};

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
            let s = status();
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
}

// ---------------------------------------------------------------- plataforma: Linux

#[cfg(unix)]
mod platform {
    use super::*;
    use std::path::{Path, PathBuf};

    // ---------------------------------------------------------------- admin

    pub fn is_elevated() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    /// No Linux nada exige root: o proxy do desktop (gsettings/dconf) e o
    /// ~/.xsessionrc pertencem ao próprio usuário — não há elevação a fazer.
    pub fn ensure_admin() {}

    // ------------------------------------------------ variáveis persistentes

    /// O XFCE/Mint inicia a sessão gráfica pelo /etc/X11/Xsession, que faz
    /// `source` deste arquivo: tudo que exportamos aqui vale para a sessão
    /// inteira no próximo login (menus, terminais, apps lançados pelo desktop).
    pub const ENV_BLOCK_BEGIN: &str = "# >>> proxy-toggle (gerado pela app; relogue para aplicar) >>>";
    pub const ENV_BLOCK_END: &str = "# <<< proxy-toggle <<<";

    fn home_dir() -> PathBuf {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn session_env_path() -> PathBuf {
        home_dir().join(".xsessionrc")
    }

    /// Remove o bloco gerado pela app, preservando o resto do arquivo.
    pub fn strip_env_block(content: &str) -> String {
        let mut out = String::new();
        let mut in_block = false;
        let mut started = false;
        for line in content.lines() {
            if !started && line.trim() == ENV_BLOCK_BEGIN {
                in_block = true;
                started = true;
                continue;
            }
            if in_block {
                if line.trim() == ENV_BLOCK_END {
                    in_block = false;
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// Grava (ou substitui) o bloco de variáveis no arquivo indicado.
    pub fn write_env_block(path: &Path, url: &str, no_proxy: &str) -> Result<(), String> {
        let current = std::fs::read_to_string(path).unwrap_or_default();
        let stripped = strip_env_block(&current);
        let mut out = String::new();
        let tail = stripped.trim_end();
        if !tail.is_empty() {
            out.push_str(tail);
            out.push_str("\n\n");
        }
        out.push_str(ENV_BLOCK_BEGIN);
        out.push('\n');
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            out.push_str(&format!("export {key}=\"{url}\"\n"));
        }
        out.push_str(&format!("export NO_PROXY=\"{no_proxy}\"\n"));
        for key in ["http_proxy", "https_proxy", "all_proxy"] {
            out.push_str(&format!("export {key}=\"{url}\"\n"));
        }
        out.push_str(&format!("export no_proxy=\"{no_proxy}\"\n"));
        out.push_str(ENV_BLOCK_END);
        out.push('\n');
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, out).map_err(|e| e.to_string())
    }

    pub fn remove_env_block(path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        let current = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let stripped = strip_env_block(&current);
        if stripped == current {
            return Ok(());
        }
        std::fs::write(path, stripped).map_err(|e| e.to_string())
    }

    // ---------------------------------------------------------------- proxy

    fn gsettings_available() -> bool {
        run("gsettings", &["get", "org.gnome.system.proxy", "mode"]).ok
    }

    /// `localhost;127.0.0.1` -> `['localhost', '127.0.0.1']` (formato do gvariant).
    fn gsettings_ignore_hosts(bypass: &str) -> String {
        let items: Vec<String> = bypass_entries(bypass)
            .iter()
            .map(|e| format!("'{e}'"))
            .collect();
        format!("[{}]", items.join(", "))
    }

    /// `['a', 'b']` -> `a;b` (aceita `@as []`, lista vazia do gsettings).
    fn parse_gvariant_strv(v: &str) -> String {
        let t = v.trim();
        if t.starts_with('@') || t == "[]" {
            return String::new();
        }
        let t = t.trim_start_matches('[').trim_end_matches(']').trim();
        if t.is_empty() {
            return String::new();
        }
        t.split(',')
            .map(|s| s.trim().trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(";")
    }

    pub fn apply_enable(host: &str, port: u16, bypass: &str) -> Result<ToggleResult, String> {
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
        let no_proxy = bypass_entries(&bypass).join(",");
        let mut steps = Vec::new();

        // 1 - proxy do desktop: XFCE (Mint), Cinnamon, MATE e GNOME leem daqui.
        let system_ok = if gsettings_available() {
            let http_schema = "org.gnome.system.proxy.http".to_string();
            let https_schema = "org.gnome.system.proxy.https".to_string();
            let ignore = gsettings_ignore_hosts(&bypass);
            let port_s = port.to_string();
            let mut ok = step(
                &mut steps,
                "proxy-mode",
                "gsettings",
                &["set", "org.gnome.system.proxy", "mode", "manual"],
            );
            ok &= step(
                &mut steps,
                "proxy-ignore",
                "gsettings",
                &["set", "org.gnome.system.proxy", "ignore-hosts", ignore.as_str()],
            );
            ok &= step(
                &mut steps,
                "proxy-http",
                "gsettings",
                &["set", http_schema.as_str(), "host", host.as_str()],
            );
            ok &= step(
                &mut steps,
                "proxy-http-port",
                "gsettings",
                &["set", http_schema.as_str(), "port", port_s.as_str()],
            );
            ok &= step(
                &mut steps,
                "proxy-https",
                "gsettings",
                &["set", https_schema.as_str(), "host", host.as_str()],
            );
            ok &= step(
                &mut steps,
                "proxy-https-port",
                "gsettings",
                &["set", https_schema.as_str(), "port", port_s.as_str()],
            );
            ok
        } else {
            inline_step(&mut steps, "gsettings", false)
        };

        // 2 - variáveis persistentes (valem a partir do próximo login)
        let env_ok = match write_env_block(&session_env_path(), &url, &no_proxy) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("proxy-toggle: {e}");
                false
            }
        };
        inline_step(&mut steps, "env-persist", env_ok);

        // 3 - cache DNS (systemd-resolved aceita sem root; se faltar, não é crítico)
        step(&mut steps, "flush-dns", "resolvectl", &["flush-caches"]);

        Ok(ToggleResult {
            ok: system_ok && env_ok,
            system_ok,
            steps,
        })
    }

    pub fn apply_disable() -> Result<ToggleResult, String> {
        let mut steps = Vec::new();

        let system_ok = if gsettings_available() {
            step(
                &mut steps,
                "proxy-mode",
                "gsettings",
                &["set", "org.gnome.system.proxy", "mode", "none"],
            )
        } else {
            inline_step(&mut steps, "gsettings", false)
        };

        let env_ok = match remove_env_block(&session_env_path()) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("proxy-toggle: {e}");
                false
            }
        };
        inline_step(&mut steps, "env-persist", env_ok);

        step(&mut steps, "flush-dns", "resolvectl", &["flush-caches"]);

        Ok(ToggleResult {
            ok: system_ok && env_ok,
            system_ok,
            steps,
        })
    }

    pub fn status() -> ProxyStatus {
        let mode = run("gsettings", &["get", "org.gnome.system.proxy", "mode"])
            .out
            .trim()
            .trim_matches('\'')
            .to_string();
        let host = run("gsettings", &["get", "org.gnome.system.proxy.http", "host"])
            .out
            .trim()
            .trim_matches('\'')
            .to_string();
        let port = run("gsettings", &["get", "org.gnome.system.proxy.http", "port"])
            .out
            .trim()
            .to_string();
        let ignore_raw = run("gsettings", &["get", "org.gnome.system.proxy", "ignore-hosts"]).out;
        let bypass = parse_gvariant_strv(&ignore_raw);
        let server = if host.is_empty() || port.is_empty() || port == "0" {
            String::new()
        } else {
            format!("{host}:{port}")
        };
        ProxyStatus {
            enabled: mode == "manual",
            server,
            bypass,
            winhttp: mode,
        }
    }

    // ---------------------------------------------------------------- rede

    /// Uma leitura de /proc/net/dev: uma syscall, sem spawn de processo.
    /// Mede TODO o tráfego das placas — incluindo o que passa pelo proxy.
    pub fn native_sample() -> Result<(u64, u64, usize), String> {
        let data = std::fs::read_to_string("/proc/net/dev").map_err(|e| e.to_string())?;
        parse_proc_net_dev(&data)
    }

    pub fn parse_proc_net_dev(data: &str) -> Result<(u64, u64, usize), String> {
        let mut rx = 0u64;
        let mut tx = 0u64;
        let mut n = 0usize;
        for line in data.lines().skip(2) {
            let mut parts = line.splitn(2, ':');
            let iface = match parts.next() {
                Some(i) => i.trim(),
                None => continue,
            };
            let rest = match parts.next() {
                Some(r) => r,
                None => continue,
            };
            if iface == "lo" {
                continue; // loopback não conta
            }
            let f: Vec<&str> = rest.split_whitespace().collect();
            if f.len() < 9 {
                continue;
            }
            rx += f[0].parse::<u64>().unwrap_or(0);
            tx += f[8].parse::<u64>().unwrap_or(0);
            n += 1;
        }
        Ok((rx, tx, n))
    }

    // ---------------------------------------------------------------- testes

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{INTERNAL_HOSTS, bypass_entries, with_internal_bypass};

        #[test]
        fn bypass_sempre_inclui_hosts_internos() {
            let b = with_internal_bypass("localhost;127.0.0.1;*.local");
            for h in INTERNAL_HOSTS {
                assert!(b.contains(h), "faltando {h} em: {b}");
            }
            let b2 = with_internal_bypass("ipc.localhost;foo.com");
            assert_eq!(b2.matches("ipc.localhost").count(), 1, "{b2}");
        }

        #[test]
        fn excecoes_aceitam_ponto_e_virgula_e_virgula() {
            assert_eq!(bypass_entries("a;b, c ;d,"), vec!["a", "b", "c", "d"]);
        }

        #[test]
        fn bloco_xsessionrc_grava_substitui_e_remove() {
            let dir = std::env::temp_dir().join(format!("proxy-toggle-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let f = dir.join(".xsessionrc");
            std::fs::write(&f, "# minha config\nexport FOO=1\n").unwrap();

            write_env_block(&f, "http://127.0.0.1:59999", "localhost,127.0.0.1").unwrap();
            let c1 = std::fs::read_to_string(&f).unwrap();
            assert!(c1.contains(ENV_BLOCK_BEGIN), "sem bloco: {c1}");
            assert!(c1.contains("export HTTP_PROXY=\"http://127.0.0.1:59999\""), "{c1}");
            assert!(c1.contains("export no_proxy=\"localhost,127.0.0.1\""), "{c1}");
            assert!(c1.contains("export FOO=1"), "conteúdo do usuário perdido: {c1}");

            // segunda gravação substitui o bloco em vez de duplicar
            write_env_block(&f, "http://10.0.0.9:1", "x").unwrap();
            let c2 = std::fs::read_to_string(&f).unwrap();
            assert_eq!(c2.matches("HTTP_PROXY").count(), 1, "bloco duplicado: {c2}");
            assert_eq!(c2.matches(ENV_BLOCK_BEGIN).count(), 1, "{c2}");

            remove_env_block(&f).unwrap();
            let c3 = std::fs::read_to_string(&f).unwrap();
            assert!(!c3.contains("PROXY"), "bloco não removido: {c3}");
            assert!(c3.contains("export FOO=1"), "conteúdo do usuário perdido: {c3}");

            // remover de novo (já limpo) não pode dar erro nem perder conteúdo
            remove_env_block(&f).unwrap();
            assert!(std::fs::read_to_string(&f).unwrap().contains("export FOO=1"));

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn strip_preserva_texto_do_usuario() {
            let content = format!("A\n{ENV_BLOCK_BEGIN}\nexport HTTP_PROXY=\"x\"\n{ENV_BLOCK_END}\nB\n");
            assert_eq!(strip_env_block(&content), "A\nB\n");
            assert_eq!(strip_env_block("só usuário\n"), "só usuário\n");
            assert_eq!(strip_env_block(""), "");
        }

        #[test]
        fn proc_net_dev_soma_sem_loopback() {
            let data = "Inter-|   Receive                                                |  Transmit\n \
                        face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n \
                        lo: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0\n \
                        wlp2s0: 500 2 0 0 0 0 0 0 50 1 0 0 0 0 0 0\n";
            let (rx, tx, n) = parse_proc_net_dev(data).unwrap();
            assert_eq!((rx, tx, n), (500, 50, 1));
        }

        #[test]
        fn gvariant_ignora_lista_vazia() {
            assert_eq!(parse_gvariant_strv("['localhost', '127.0.0.1']"), "localhost;127.0.0.1");
            assert_eq!(parse_gvariant_strv("[]"), "");
            assert_eq!(parse_gvariant_strv("@as []"), "");
        }
    }
}

// ---------------------------------------------------------------- comandos tauri

#[tauri::command]
fn is_admin() -> bool {
    platform::is_elevated()
}

#[tauri::command]
fn enable_proxy(cfg: State<LastCfg>, host: String, port: u16, bypass: String) -> Result<ToggleResult, String> {
    let r = platform::apply_enable(&host, port, &bypass)?;
    if let Ok(mut g) = cfg.0.lock() {
        *g = (host.trim().to_string(), port, bypass.trim().to_string());
    }
    Ok(r)
}

#[tauri::command]
fn disable_proxy() -> Result<ToggleResult, String> {
    platform::apply_disable()
}

#[tauri::command]
fn get_status() -> ProxyStatus {
    platform::status()
}

// ---------------------------------------------------------------- rede

static PREV: Mutex<Option<(u64, u64, u64)>> = Mutex::new(None);

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
    #[cfg(windows)]
    let (rx, tx, adapters) = match platform::native_sample() {
        Ok(v) => v,
        Err(_) => platform::ps_sample()?,
    };
    #[cfg(unix)]
    let (rx, tx, adapters) = platform::native_sample()?;

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
                    let _ = platform::apply_enable(&cfg.0, cfg.1, &cfg.2);
                    let _ = h.emit("proxy-changed", ());
                });
            }
            "off" => {
                let h = app.clone();
                std::thread::spawn(move || {
                    let _ = platform::apply_disable();
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
    platform::ensure_admin();
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
