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
    pub detecting: &'static str,
    pub not_installed: &'static str,
    pub install: &'static str,
    pub inactive: &'static str,
    pub start_service: &'static str,
    pub unreachable: &'static str,
    pub working: &'static str,
    pub last_error: &'static str,
    pub phase_running: &'static str,
    pub phase_stopped: &'static str,
    pub phase_starting: &'static str,
    pub phase_stopping: &'static str,
    pub phase_recovering: &'static str,
    pub phase_failed: &'static str,
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
    pub start_core: &'static str,
    pub stop_core: &'static str,
    pub restart_core: &'static str,
    pub open_dashboard: &'static str,
    pub service_page: &'static str,
    pub autostart: &'static str,
    pub quit: &'static str,
    pub timeout: &'static str,
}

const EN: Strings = Strings {
    detecting: "Detecting mihomo-server…",
    not_installed: "mihomo-server is not installed",
    install: "Install mihomo-server…",
    inactive: "mihomo-server is not running",
    start_service: "Start service",
    unreachable: "Cannot reach mihomo-server: {reason}",
    working: "Working…",
    last_error: "Failed: {reason}",
    phase_running: "Core running",
    phase_stopped: "Core stopped",
    phase_starting: "Core starting",
    phase_stopping: "Core stopping",
    phase_recovering: "Core recovering",
    phase_failed: "Core failed",
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
    start_core: "Start core",
    stop_core: "Stop core",
    restart_core: "Restart core",
    open_dashboard: "Open dashboard",
    service_page: "Service status…",
    autostart: "Start at login",
    quit: "Quit",
    timeout: "timeout",
};

const ZH: Strings = Strings {
    detecting: "正在检测 mihomo-server…",
    not_installed: "未安装 mihomo-server",
    install: "安装 mihomo-server…",
    inactive: "mihomo-server 未运行",
    start_service: "启动服务",
    unreachable: "无法连接 mihomo-server：{reason}",
    working: "正在处理…",
    last_error: "操作失败：{reason}",
    phase_running: "内核运行中",
    phase_stopped: "内核已停止",
    phase_starting: "内核启动中",
    phase_stopping: "内核停止中",
    phase_recovering: "内核恢复中",
    phase_failed: "内核异常",
    direct: "直连",
    rule: "规则",
    global: "全局",
    tun: "TUN 模式",
    tun_held: "TUN 模式（{name} 正在使用）",
    tun_unavailable: "TUN 模式（无权限）",
    groups: "代理分组",
    automatic: "自动选择",
    test_delay: "测试延迟",
    more_nodes: "另有 {count} 个节点，请在管理界面中查看…",
    profiles: "订阅",
    start_core: "启动内核",
    stop_core: "停止内核",
    restart_core: "重启内核",
    open_dashboard: "打开管理界面",
    service_page: "服务状态…",
    autostart: "登录时启动",
    quit: "退出",
    timeout: "超时",
};

const ZHTW: Strings = Strings {
    detecting: "正在偵測 mihomo-server…",
    not_installed: "未安裝 mihomo-server",
    install: "安裝 mihomo-server…",
    inactive: "mihomo-server 未執行",
    start_service: "啟動服務",
    unreachable: "無法連線 mihomo-server：{reason}",
    working: "正在處理…",
    last_error: "操作失敗：{reason}",
    phase_running: "核心執行中",
    phase_stopped: "核心已停止",
    phase_starting: "核心啟動中",
    phase_stopping: "核心停止中",
    phase_recovering: "核心復原中",
    phase_failed: "核心異常",
    direct: "直連",
    rule: "規則",
    global: "全域",
    tun: "TUN 模式",
    tun_held: "TUN 模式（{name} 正在使用）",
    tun_unavailable: "TUN 模式（無權限）",
    groups: "代理群組",
    automatic: "自動選擇",
    test_delay: "測試延遲",
    more_nodes: "另有 {count} 個節點，請在管理介面中查看…",
    profiles: "訂閱",
    start_core: "啟動核心",
    stop_core: "停止核心",
    restart_core: "重新啟動核心",
    open_dashboard: "開啟管理介面",
    service_page: "服務狀態…",
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
