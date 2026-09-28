// Browser language stays in this origin's storage; it never changes the
// service-wide locale. The fallback and regional aliases follow upstream i18n.
export type Language = "zh" | "en";
const STORAGE_KEY = "mihomo-server-language";

export function resolveLanguage(value?: string | null): Language {
  const normalized = value?.toLowerCase().replaceAll("_", "-") || "";
  if (normalized === "en" || normalized.startsWith("en-")) return "en";
  return "zh";
}

export function savedLanguage(): Language {
  try { return resolveLanguage(window.localStorage.getItem(STORAGE_KEY)); }
  catch { return "zh"; }
}

export function saveLanguage(language: Language) {
  try { window.localStorage.setItem(STORAGE_KEY, language); }
  catch { /* Private browser storage can be unavailable. */ }
}

const zh = {
  language: "界面语言", navLabel: "工作空间", navAria: "主导航",
  overview: "概览", profiles: "订阅", config: "配置", proxies: "节点", rules: "规则",
  logs: "日志", settings: "设置", core: "内核升级", logout: "退出登录",
  serviceManagement: "服务管理", eventConnection: "事件连接状态",
  stopped: "已停止", starting: "启动中", running: "运行中", stopping: "停止中",
  recovering: "恢复中", failed: "启动失败", shutdown: "服务关闭",
  connecting: "连接中", connected: "已连接", reconnecting: "重连中",
  unauthorized: "认证失败", badData: "数据错误",
  coreName: "Mihomo 内核", noProfile: "未选择订阅", coreStopped: "内核未运行",
  startCore: "启动内核", stopCore: "停止内核", restartCore: "重启内核",
  working: "正在处理，请稍候…", completed: "操作已完成",
  coreError: "内核错误", operationError: "上次操作错误",
  selectionRestore: "节点恢复", restoringNodes: "正在恢复节点",
  footer: "独立运行 · 配置与节点选择由服务保存",
  loginEyebrow: "独立服务 · 浏览器管理", loginTitleFirst: "让连接",
  loginTitleSecond: "尽在掌握。", loginIntroFirst: "订阅、配置与内核状态，",
  loginIntroSecond: "在同一个地方管理。", loginEntry: "服务管理入口",
  loginTitle: "连接你的服务", loginHelp: "输入服务管理令牌以继续。令牌仅保存在当前页面内存，刷新后需重新登录。",
  token: "管理令牌", verifying: "验证中…", connect: "连接服务",
  tokenHint: "令牌位于服务数据目录的 management-token 文件中。",
  invalidToken: "令牌无效，请检查后重试。", expiredToken: "认证失效，请重新输入令牌。",
  uploadRate: "上传速率", downloadRate: "下载速率", activeConnections: "活跃连接", memory: "内存占用",
  currentConfig: "当前配置", editConfig: "编辑配置 ↗", activeProfile: "活动订阅",
  notSelected: "未选择", configVersion: "配置版本", notCommitted: "尚未提交",
  coreStatus: "内核状态", configHint: "修改会先经过 YAML 和内核校验。失败时保留上一个已提交配置。",
  getStarted: "开始使用", manageProfiles: "管理订阅 ↗",
  stepImport: "导入本地 YAML 订阅", stepUse: "使用订阅并启动内核", stepSelect: "选择节点，设置会自动保存",
  recentLogs: "最近日志", viewAll: "查看全部 ↗",
} as const;

const en: Record<keyof typeof zh, string> = {
  language: "Interface language", navLabel: "Workspace", navAria: "Main navigation",
  overview: "Overview", profiles: "Profiles", config: "Configuration", proxies: "Proxies", rules: "Rules",
  logs: "Logs", settings: "Settings", core: "Core upgrade", logout: "Log out",
  serviceManagement: "Service management", eventConnection: "Event connection status",
  stopped: "Stopped", starting: "Starting", running: "Running", stopping: "Stopping",
  recovering: "Recovering", failed: "Start failed", shutdown: "Service closed",
  connecting: "Connecting", connected: "Connected", reconnecting: "Reconnecting",
  unauthorized: "Authentication failed", badData: "Invalid data",
  coreName: "Mihomo core", noProfile: "No profile selected", coreStopped: "Core not running",
  startCore: "Start core", stopCore: "Stop core", restartCore: "Restart core",
  working: "Working, please wait…", completed: "Operation completed",
  coreError: "Core error", operationError: "Previous operation error",
  selectionRestore: "Node restoration", restoringNodes: "Restoring nodes",
  footer: "Standalone service · Configuration and node selection are saved by the service",
  loginEyebrow: "Standalone service · Browser management", loginTitleFirst: "Your connection,",
  loginTitleSecond: "under control.", loginIntroFirst: "Manage profiles, configuration,",
  loginIntroSecond: "and core status in one place.", loginEntry: "Management access",
  loginTitle: "Connect to your service", loginHelp: "Enter the management token to continue. It stays in this page's memory and is cleared on refresh.",
  token: "Management token", verifying: "Verifying…", connect: "Connect to service",
  tokenHint: "Find the token in the management-token file in the service data directory.",
  invalidToken: "Invalid token. Check it and try again.", expiredToken: "Authentication expired. Enter the token again.",
  uploadRate: "Upload rate", downloadRate: "Download rate", activeConnections: "Active connections", memory: "Memory use",
  currentConfig: "Current configuration", editConfig: "Edit configuration ↗", activeProfile: "Active profile",
  notSelected: "None selected", configVersion: "Configuration revision", notCommitted: "Not committed",
  coreStatus: "Core status", configHint: "Changes are checked against YAML and the core before application. Failures keep the last committed configuration.",
  getStarted: "Get started", manageProfiles: "Manage profiles ↗",
  stepImport: "Import a local YAML profile", stepUse: "Activate the profile and start the core", stepSelect: "Select a node; the service saves your choice",
  recentLogs: "Recent logs", viewAll: "View all ↗",
};

export type MessageKey = keyof typeof zh;
export function t(language: Language, key: MessageKey): string {
  return language === "en" ? en[key] : zh[key];
}

const phaseKeys: Record<string, MessageKey> = {
  stopped: "stopped", starting: "starting", running: "running", stopping: "stopping",
  recovering: "recovering", failed: "failed", shutdown: "shutdown",
};
export function phaseLabel(language: Language, phase: string): string {
  return phaseKeys[phase] ? t(language, phaseKeys[phase]) : phase;
}

const connectionKeys: Record<string, MessageKey> = {
  "连接中": "connecting", "已连接": "connected", "重连中": "reconnecting",
  "认证失败": "unauthorized", "数据错误": "badData",
};
export function connectionLabel(language: Language, connection: string): string {
  return connectionKeys[connection] ? t(language, connectionKeys[connection]) : connection;
}
