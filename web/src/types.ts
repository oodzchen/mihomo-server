export interface CoreStatus {
  phase: string;
  pid?: number;
  version?: string;
  error?: string;
  generation: number;
  config_revision?: string;
  active_profile?: string;
  selection_pending: string[];
  selection_error?: string;
}
export interface Profile {
  uid: string;
  file?: string;
  name?: string;
  type?: string;
  desc?: string;
  url?: string;
  option?: {
    merge?: string;
    script?: string;
    rules?: string;
    proxies?: string;
    groups?: string;
    user_agent?: string;
    timeout_seconds?: number;
    update_interval?: number;
    allow_auto_update?: boolean;
    self_proxy?: boolean;
    with_proxy?: boolean;
    danger_accept_invalid_certs?: boolean;
  };
  extra?: { upload: number; download: number; total: number; expire: number };
  selected?: { name?: string; now?: string }[];
}
export interface Profiles {
  current?: string;
  items?: Profile[];
}
export interface CoreLog {
  stream: string;
  message: string;
}
export interface ProxyHistory {
  time: string;
  delay: number;
}
export interface Proxy {
  name: string;
  type: string;
  all?: string[];
  now?: string;
  fixed?: string;
  history?: ProxyHistory[];
  alive?: boolean;
}
export interface Proxies {
  proxies: Record<string, Proxy>;
}
export interface ProxyProvider {
  name: string;
  type: string;
  vehicleType: string;
  proxies?: Proxy[];
  testUrl?: string;
  expectedStatus?: string;
  updatedAt?: string;
  subscriptionInfo?: {
    upload: number;
    download: number;
    total: number;
    expire: number;
  };
}
export interface ProxyProviders {
  providers: Record<string, ProxyProvider>;
}
export interface ProxyDelay {
  delay: number;
}
/** Interface preferences this instance shares with all of its clients. */
export type Preferences = { language?: "zh" | "zhtw" | "en" | null };
export type EventMessage = {
  type: string;
  data?: unknown;
  status?: CoreStatus;
  profiles?: Profiles;
  logs?: CoreLog[];
  preferences?: Preferences;
  code?: string;
  message?: string;
};
export interface Rule {
  type: string;
  payload: string;
  proxy: string;
  size?: number;
}
export interface Rules {
  rules: Rule[];
}
export interface RuleProvider {
  name: string;
  type: string;
  vehicleType: string;
  behavior: string;
  format: string;
  ruleCount: number;
  updatedAt: string;
}
export interface RuleProviders {
  providers: Record<string, RuleProvider>;
}

/** The latest recorded update checks; each names the version installed then. */
export type UpdateChecks = {
  core: { channel: "stable" | "alpha"; installed: string; latest: string } | null;
  service: { installed: string | null; latest: string } | null;
};
