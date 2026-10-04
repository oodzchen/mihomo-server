"use strict";
// Local status page: detect, install or start this user's mihomo-server.
const invoke = (command, args) => window.__TAURI_INTERNALS__.invoke(command, args);

const TEXT = {
  en: {
    detecting: ["Detecting mihomo-server…", "Checking this user's service."],
    not_installed: ["mihomo-server is not installed",
      "Install downloads the official installer and sets up the latest release. Your system will ask for administrator authorization."],
    inactive: ["mihomo-server is not running", "The service is installed but stopped. Start it to manage proxies."],
    unreachable: ["Cannot reach mihomo-server", ""],
    running: ["mihomo-server is running", ""],
    install: "Install", installing: "Installing…", start: "Start service", starting: "Starting…",
    open: "Open dashboard", retry: "Waiting…", log: "Output", empty: "No output yet.",
  },
  zh: {
    detecting: ["正在检测 mihomo-server…", "正在检查当前用户的服务。"],
    not_installed: ["未安装 mihomo-server", "点击安装将下载官方安装脚本并安装最新版本，安装过程中系统会请求管理员授权。"],
    inactive: ["mihomo-server 未运行", "服务已安装但未启动，启动后即可管理代理。"],
    unreachable: ["无法连接 mihomo-server", ""],
    running: ["mihomo-server 运行中", ""],
    install: "安装", installing: "正在安装…", start: "启动服务", starting: "正在启动…",
    open: "打开管理界面", retry: "等待中…", log: "输出", empty: "暂无输出。",
  },
  zhtw: {
    detecting: ["正在偵測 mihomo-server…", "正在檢查目前使用者的服務。"],
    not_installed: ["未安裝 mihomo-server", "點擊安裝將下載官方安裝腳本並安裝最新版本，安裝過程中系統會要求管理員授權。"],
    inactive: ["mihomo-server 未執行", "服務已安裝但未啟動，啟動後即可管理代理。"],
    unreachable: ["無法連線 mihomo-server", ""],
    running: ["mihomo-server 執行中", ""],
    install: "安裝", installing: "正在安裝…", start: "啟動服務", starting: "正在啟動…",
    open: "開啟管理介面", retry: "等待中…", log: "輸出", empty: "尚無輸出。",
  },
};
const DOT = { running: "ok", inactive: "warn", not_installed: "warn", unreachable: "bad", detecting: "" };

const $ = (id) => document.getElementById(id);
let current, lastLog = "";

function render(state) {
  current = state;
  const text = TEXT[state.language] || TEXT.en;
  document.documentElement.lang = state.language === "en" ? "en" : state.language === "zhtw" ? "zh-TW" : "zh-CN";
  const [title, description] = text[state.state] || text.detecting;
  $("title").textContent = title;
  $("dot").className = "dot " + (DOT[state.state] || "");
  $("detail").textContent = state.state === "running"
    ? [state.address, state.version].filter(Boolean).join(" · ")
    : state.detail || description;
  $("error").textContent = state.last_error || "";
  $("app-version").textContent = "v" + state.app_version;
  $("log-title").textContent = text.log;

  const button = $("primary"), busy = state.task !== "idle";
  if (state.task === "installing") button.textContent = text.installing;
  else if (state.task === "starting") button.textContent = text.starting;
  else if (state.state === "not_installed") button.textContent = text.install;
  else if (state.state === "inactive") button.textContent = text.start;
  else if (state.state === "running") button.textContent = text.open;
  else button.textContent = text.retry;
  button.disabled = busy || !["not_installed", "inactive", "running"].includes(state.state);

  const log = $("log"), joined = state.log.join("\n") || text.empty;
  if (joined !== lastLog) {
    const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 24;
    log.textContent = joined;
    lastLog = joined;
    if (atBottom) log.scrollTop = log.scrollHeight;
  }
}

async function refresh() {
  try { render(await invoke("local_state")); }
  catch (error) { $("error").textContent = String(error); }
}

$("primary").addEventListener("click", async () => {
  if (!current) return;
  const command = { not_installed: "install_service", inactive: "start_service", running: "open_dashboard" }[current.state];
  if (!command) return;
  $("primary").disabled = true;
  try { await invoke(command); }
  catch (error) { $("error").textContent = String(error); }
  refresh();
});

refresh();
setInterval(refresh, 1000);
