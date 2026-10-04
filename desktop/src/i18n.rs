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
    pub desktop: &'static str,
    pub service: &'static str,
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
    desktop: "Desktop client v{version}",
    service: "Service",
    service_detecting: "detecting…",
    service_not_installed: "not installed",
    service_installing: "installing…",
    service_stopped: "stopped",
    service_starting: "starting…",
    service_stopping: "stopping…",
    service_restarting: "restarting…",
    service_running: "running",
    service_unreachable: "unreachable",
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
    quit: "Quit",
    timeout: "timeout",
};

const ZH: Strings = Strings {
    desktop: "桌面端 v{version}",
    service: "服务",
    service_detecting: "检测中…",
    service_not_installed: "未安装",
    service_installing: "安装中…",
    service_stopped: "未运行",
    service_starting: "启动中…",
    service_stopping: "停止中…",
    service_restarting: "重启中…",
    service_running: "运行中",
    service_unreachable: "无法连接",
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
    quit: "退出",
    timeout: "超时",
};

const ZHTW: Strings = Strings {
    desktop: "桌面端 v{version}",
    service: "服務",
    service_detecting: "偵測中…",
    service_not_installed: "未安裝",
    service_installing: "安裝中…",
    service_stopped: "未執行",
    service_starting: "啟動中…",
    service_stopping: "停止中…",
    service_restarting: "重新啟動中…",
    service_running: "執行中",
    service_unreachable: "無法連線",
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
    quit: "結束",
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
