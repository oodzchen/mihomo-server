//! Tray and status-page strings for the system locale; no runtime switching.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    Zh,
    Zhtw,
}

impl Language {
    /// Same aliases as the service catalog: zh-TW/HK/MO and Hant are traditional.
    pub fn from_locale(locale: &str) -> Self {
        let locale = locale.to_ascii_lowercase().replace('_', "-");
        if !locale.starts_with("zh") {
            return Self::En;
        }
        if ["-tw", "-hk", "-mo", "-hant"].iter().any(|tag| locale.contains(tag)) {
            Self::Zhtw
        } else {
            Self::Zh
        }
    }

    /// A language the service shares as an instance preference.
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "en" => Some(Self::En),
            "zh" => Some(Self::Zh),
            "zhtw" => Some(Self::Zhtw),
            _ => None,
        }
    }

    pub fn system() -> Self {
        Self::from_locale(&sys_locale::get_locale().unwrap_or_default())
    }

    /// `Accept-Language` for service messages.
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Zh => "zh",
            Self::Zhtw => "zhtw",
        }
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Self::En => &EN,
            Self::Zh => &ZH,
            Self::Zhtw => &ZHTW,
        }
    }
}

pub struct Strings {
    pub service_detecting: &'static str,
    pub service_not_installed: &'static str,
    pub service_installing: &'static str,
    pub service_stopped: &'static str,
    pub service_starting: &'static str,
    pub service_stopping: &'static str,
    pub service_restarting: &'static str,
    pub service_running: &'static str,
    pub service_unreachable: &'static str,
    pub install: &'static str,
    pub start_service: &'static str,
    pub stop_service: &'static str,
    pub restart_service: &'static str,
    pub working: &'static str,
    pub failed: &'static str,
    pub mode: &'static str,
    pub direct: &'static str,
    pub rule: &'static str,
    pub global: &'static str,
    pub tun: &'static str,
    pub tun_held: &'static str,
    pub tun_unavailable: &'static str,
    pub groups: &'static str,
    pub automatic: &'static str,
    pub test_delay: &'static str,
    pub more_nodes: &'static str,
    pub profiles: &'static str,
    pub open_dashboard: &'static str,
    pub autostart: &'static str,
    pub quit: &'static str,
    pub timeout: &'static str,
}

const EN: Strings = Strings {
    service_detecting: "Detecting service…",
    service_not_installed: "Service not installed",
    service_installing: "Installing service…",
    service_stopped: "Service stopped",
    service_starting: "Starting service…",
    service_stopping: "Stopping service…",
    service_restarting: "Restarting service…",
    service_running: "Service running",
    service_unreachable: "Service unreachable",
    install: "Install mihomo-server service…",
    start_service: "Start service",
    stop_service: "Stop service",
    restart_service: "Restart service",
    working: "Working…",
    failed: "Operation failed",
    mode: "Proxy mode",
    direct: "Direct",
    rule: "Rule",
    global: "Global",
    tun: "TUN mode",
    tun_held: "TUN mode (held by {name})",
    tun_unavailable: "TUN mode (not permitted)",
    groups: "Proxy groups",
    automatic: "Automatic",
    test_delay: "Test latency",
    more_nodes: "{count} more in the dashboard…",
    profiles: "Subscriptions",
    open_dashboard: "Open dashboard",
    autostart: "Start at login",
    quit: "Quit client",
    timeout: "timeout",
};

const ZH: Strings = Strings {
    service_detecting: "正在检测服务…",
    service_not_installed: "服务未安装",
    service_installing: "正在安装服务…",
    service_stopped: "服务未运行",
    service_starting: "正在启动服务…",
    service_stopping: "正在停止服务…",
    service_restarting: "正在重启服务…",
    service_running: "服务运行中",
    service_unreachable: "服务无法连接",
    install: "安装 mihomo-server 服务…",
    start_service: "启动服务",
    stop_service: "停止服务",
    restart_service: "重启服务",
    working: "正在处理…",
    failed: "操作失败",
    mode: "代理模式",
    direct: "直连",
    rule: "规则",
    global: "全局",
    tun: "TUN 模式",
    tun_held: "TUN 模式（{name} 正在使用）",
    tun_unavailable: "TUN 模式（无权限）",
    groups: "代理分组",
    automatic: "自动选择",
    test_delay: "测试延迟",
    more_nodes: "另有 {count} 个节点，请在管理程序中查看…",
    profiles: "订阅",
    open_dashboard: "打开管理程序",
    autostart: "登录时启动",
    quit: "退出客户端",
    timeout: "超时",
};

const ZHTW: Strings = Strings {
    service_detecting: "正在偵測服務…",
    service_not_installed: "服務未安裝",
    service_installing: "正在安裝服務…",
    service_stopped: "服務未執行",
    service_starting: "正在啟動服務…",
    service_stopping: "正在停止服務…",
    service_restarting: "正在重新啟動服務…",
    service_running: "服務執行中",
    service_unreachable: "服務無法連線",
    install: "安裝 mihomo-server 服務…",
    start_service: "啟動服務",
    stop_service: "停止服務",
    restart_service: "重新啟動服務",
    working: "正在處理…",
    failed: "操作失敗",
    mode: "代理模式",
    direct: "直連",
    rule: "規則",
    global: "全域",
    tun: "TUN 模式",
    tun_held: "TUN 模式（{name} 正在使用）",
    tun_unavailable: "TUN 模式（無權限）",
    groups: "代理群組",
    automatic: "自動選擇",
    test_delay: "測試延遲",
    more_nodes: "另有 {count} 個節點，請在管理程式中查看…",
    profiles: "訂閱",
    open_dashboard: "開啟管理程式",
    autostart: "登入時啟動",
    quit: "結束用戶端",
    timeout: "逾時",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_map_like_the_service_catalog() {
        assert_eq!(Language::from_locale("zh-CN"), Language::Zh);
        assert_eq!(Language::from_locale("zh_SG"), Language::Zh);
        assert_eq!(Language::from_locale("zh-Hant-TW"), Language::Zhtw);
        assert_eq!(Language::from_locale("zh_HK"), Language::Zhtw);
        assert_eq!(Language::from_locale("en-US"), Language::En);
        assert_eq!(Language::from_locale(""), Language::En);
    }
}
