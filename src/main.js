import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const $ = (id) => document.getElementById(id);
const inputs = ["host", "port", "bypass"].map($);
const els = {
  host: $("host"),
  port: $("port"),
  bypass: $("bypass"),
  power: $("power"),
  powerLabel: $("power-label"),
  dot: $("dot"),
  stateLine: $("state-line"),
  down: $("down"),
  up: $("up"),
  ping: $("ping"),
  pubip: $("pubip"),
  spark: $("spark"),
  netRefresh: $("net-refresh"),
  toast: $("toast"),
};

let connected = false;
let busy = false;
let histDown = new Array(60).fill(0);
let histUp = new Array(60).fill(0);
let toastTimer = null;

try {
  const saved = JSON.parse(localStorage.getItem("proxy-cfg") || "{}");
  if (saved.host) els.host.value = saved.host;
  if (saved.port) els.port.value = saved.port;
  if (saved.bypass) els.bypass.value = saved.bypass;
} catch { /* usa padrão */ }

function saveCfg() {
  localStorage.setItem(
    "proxy-cfg",
    JSON.stringify({ host: els.host.value.trim(), port: els.port.value.trim(), bypass: els.bypass.value.trim() })
  );
}

function toast(msg, err = false) {
  els.toast.textContent = msg;
  els.toast.classList.toggle("err", err);
  els.toast.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => els.toast.classList.remove("show"), 2600);
}

function paint() {
  els.dot.classList.toggle("on", connected);
  els.power.classList.toggle("on", connected);
  els.power.disabled = busy;
  for (const el of inputs) el.disabled = busy;
  const target = connected ? `${els.host.value.trim()}:${els.port.value.trim()}` : "Desligado";
  els.stateLine.textContent = busy ? "Aplicando…" : connected ? `Ligado · ${target}` : "Desligado";
  els.powerLabel.textContent = busy ? "Aguarde" : connected ? "Toque para desligar" : "Toque para ligar";
}

function fmtRate(bps) {
  if (!isFinite(bps) || bps < 0) return "—";
  if (bps < 1024) return `${bps.toFixed(0)} B/s`;
  if (bps < 1024 * 1024) return `${(bps / 1024).toFixed(1)} KB/s`;
  return `${(bps / 1024 / 1024).toFixed(2)} MB/s`;
}

async function refreshStatus(silent = true) {
  try {
    const s = await invoke("get_status");
    connected = !!s.enabled;
    if (s.enabled && s.server) {
      const [h, p] = s.server.split(":");
      if (h && document.activeElement !== els.host) els.host.value = h;
      if (p && document.activeElement !== els.port) els.port.value = p;
    }
  } catch (e) {
    if (!silent) toast(String(e), true);
  }
  paint();
}

els.power.addEventListener("click", async () => {
  if (busy) return;
  busy = true;
  paint();
  saveCfg();
  try {
    if (!connected) {
      const r = await invoke("enable_proxy", {
        host: els.host.value.trim(),
        port: Number(els.port.value),
        bypass: els.bypass.value.trim() || "localhost;127.0.0.1;*.local",
      });
      if (!r.ok) {
        const bad = (r.steps || []).filter((s) => !s.ok).map((s) => s.name + (s.timed_out ? " (tempo esgotado)" : "")).join(", ");
        toast(bad ? `Falha em: ${bad}.` : "Falha ao aplicar o proxy.", true);
      } else if (!r.winhttp_ok) toast("Ligado, mas o WinHTTP falhou.", true);
      else toast("Proxy ligado.");
    } else {
      const r = await invoke("disable_proxy");
      if (!r.ok) {
        const bad = (r.steps || []).filter((s) => !s.ok).map((s) => s.name + (s.timed_out ? " (tempo esgotado)" : "")).join(", ");
        toast(bad ? `Falha em: ${bad}.` : "Falha ao desligar o proxy.", true);
      } else toast("Proxy desligado.");
    }
  } catch (e) {
    toast(typeof e === "string" ? e : e?.message || String(e), true);
  } finally {
    busy = false;
    await refreshStatus();
  }
});

// Enter nos campos = ligar/desligar
for (const el of inputs) {
  el.addEventListener("keydown", (e) => {
    if (e.key === "Enter") els.power.click();
  });
}

function drawSpark() {
  const c = els.spark;
  const ctx = c.getContext("2d");
  const W = c.width;
  const H = c.height;
  ctx.clearRect(0, 0, W, H);
  const max = Math.max(2048, ...histDown, ...histUp);
  const line = (arr, color, fill) => {
    ctx.beginPath();
    arr.forEach((v, i) => {
      const x = (i / (arr.length - 1)) * W;
      const y = H - 3 - Math.min(1, v / max) * (H - 8);
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    });
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.5;
    ctx.stroke();
    if (fill) {
      ctx.lineTo(W, H);
      ctx.lineTo(0, H);
      ctx.closePath();
      ctx.fillStyle = fill;
      ctx.fill();
    }
  };
  line(histDown, "#4cc2ff", "rgba(76,194,255,0.10)");
  line(histUp, "#6ccb5f", null);
}

let polling = false;
async function pollTraffic() {
  if (polling) return; // nunca acumula chamadas
  polling = true;
  try {
    const s = await invoke("net_sample");
    histDown.push(s.rx_rate || 0);
    histDown.shift();
    histUp.push(s.tx_rate || 0);
    histUp.shift();
    els.down.textContent = fmtRate(s.rx_rate || 0);
    els.up.textContent = fmtRate(s.tx_rate || 0);
    drawSpark();
  } catch {
    /* próximo ciclo tenta de novo */
  } finally {
    polling = false;
  }
}

async function refreshNetInfo() {
  els.netRefresh.disabled = true;
  try {
    // Medição HTTP: passa pelo proxy do sistema quando ligado (o ping ICMP não passa).
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 8000);
    const t0 = performance.now();
    const ip = await fetch("https://api.ipify.org?format=json", { signal: ctrl.signal })
      .then((r) => r.json())
      .then((j) => ({ ok: true, ip: j.ip }))
      .catch(() => ({ ok: false }))
      .finally(() => clearTimeout(timer));
    els.ping.textContent = ip.ok ? `${Math.round(performance.now() - t0)} ms` : "—";
    els.ping.title = ip.ok ? "Latência via proxy (quando ligado)" : "sem conexão";
    els.pubip.textContent = ip.ok ? `ip ${ip.ip}` : "ip —";
  } finally {
    els.netRefresh.disabled = false;
  }
}

els.netRefresh.addEventListener("click", refreshNetInfo);

(async function init() {
  paint();
  drawSpark();
  try { await listen("proxy-changed", () => refreshStatus()); } catch { /* fora da bandeja */ }
  await refreshStatus();
  refreshNetInfo();
  setInterval(pollTraffic, 1000);
  pollTraffic();
})();
