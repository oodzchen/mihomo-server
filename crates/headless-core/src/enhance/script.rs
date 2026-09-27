//! Boa evaluation adapted from the pinned upstream script processor.
//! Run only inside the service's disposable, resource-limited script worker.
use super::field::{use_lowercase, use_lowercase_owned};
use anyhow::{Result, ensure};
use boa_engine::{Context, JsString, JsValue, Source, js_string, native_function::NativeFunction, property::Attribute};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Mapping;
use std::sync::{Arc, Mutex};

pub const MAX_SCRIPT_BYTES: usize = 1024 * 1024;
pub const MAX_JSON_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_IPC_BYTES: usize = 20 * 1024 * 1024;
const MAX_OUTPUTS: usize = 1000;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptRequest {
    pub source: String,
    pub config: Mapping,
    pub name: String,
    #[serde(default)]
    pub check_only: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptResponse {
    pub config: Option<Mapping>,
    pub logs: Vec<(String, String)>,
    pub error: Option<String>,
}

pub fn validate_source(source: &str) -> Result<()> {
    ensure!(!source.trim().is_empty(), "script source must be nonempty");
    ensure!(source.len() <= MAX_SCRIPT_BYTES, "script source exceeds 1 MiB");
    Ok(())
}

fn script_program(source: &str) -> String {
    format!(
        r#"(() => {{
            const parse = JSON.parse, stringify = JSON.stringify, isArray = Array.isArray, promise = Promise;
            {source};
            const result = main(parse(globalThis.__verge_config__), globalThis.__verge_name__);
            if (result === null || typeof result !== "object" || isArray(result) || result instanceof promise)
                throw new TypeError("main must return a synchronous configuration object");
            return stringify(result);
        }})()"#,
    )
}

pub fn evaluate(request: ScriptRequest) -> ScriptResponse {
    let logs = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let evaluated = (|| -> Result<Mapping> {
        validate_source(&request.source)?;
        ensure!(request.name.len() <= 1024, "script name exceeds 1024 bytes");
        let json = serde_json::to_string(&use_lowercase(&request.config))?;
        ensure!(json.len() <= MAX_JSON_BYTES, "script input exceeds 10 MiB JSON");
        let mut context = Context::default();
        context.runtime_limits_mut().set_loop_iteration_limit(10_000_000);
        let code = script_program(&request.source);
        if request.check_only {
            // Parse the exact program used for execution without
            // running top-level statements or invoking profile-dependent main.
            boa_engine::Script::parse(Source::from_bytes(&code), None, &mut context)
                .map_err(|error| anyhow::anyhow!("script syntax: {error}"))?;
            return Ok(request.config);
        }
        let captured = Arc::clone(&logs);
        // SAFETY: captures contain only Rust strings/counters, never Boa GC values.
        let logger = unsafe {
            NativeFunction::from_closure(move |_, args, context| {
                let mut values = Vec::new();
                for arg in args.iter().take(2) {
                    values.push(arg.to_string(context)?.to_std_string_escaped());
                }
                let mut logs = captured.lock().unwrap();
                let size = logs.iter().map(|(level, text)| level.len() + text.len()).sum::<usize>();
                let added = values.iter().map(String::len).sum::<usize>();
                if values.len() != 2 || logs.len() >= MAX_OUTPUTS || added > MAX_OUTPUT_BYTES.saturating_sub(size) {
                    return Err(boa_engine::JsError::from_opaque(
                        JsString::from("script console output limit exceeded").into(),
                    ));
                }
                logs.push((values.remove(0), values.remove(0)));
                Ok(JsValue::undefined())
            })
        };
        context
            .register_global_builtin_callable(js_string!("__verge_log__"), 2, logger)
            .map_err(|error| anyhow::anyhow!("script console setup: {error}"))?;
        context
            .register_global_property(
                js_string!("__verge_config__"),
                JsString::from(json),
                Attribute::READONLY,
            )
            .map_err(|error| anyhow::anyhow!("script config setup: {error}"))?;
        context
            .register_global_property(
                js_string!("__verge_name__"),
                JsString::from(request.name),
                Attribute::READONLY,
            )
            .map_err(|error| anyhow::anyhow!("script name setup: {error}"))?;
        context
            .eval(Source::from_bytes(
                r#"
            var console = Object.freeze(Object.fromEntries(
              ["log", "info", "error", "debug", "warn", "table"].map(level =>
                [level, data => __verge_log__(level, JSON.stringify(data, null, 2))])));
        "#,
            ))
            .map_err(|error| anyhow::anyhow!("script console setup: {error}"))?;
        // Bind data separately; names/config cannot become injected program source.
        let value = context
            .eval(Source::from_bytes(&code))
            .map_err(|error| anyhow::anyhow!("script execution: {error}"))?;
        let result = value
            .as_string()
            .ok_or_else(|| anyhow::anyhow!("main must return a configuration object"))?
            .to_std_string_escaped();
        ensure!(result.len() <= MAX_JSON_BYTES, "script result exceeds 10 MiB JSON");
        let config: Mapping = serde_json::from_str(&result)?;
        let config = use_lowercase_owned(config);
        ensure!(
            serde_yaml_ng::to_string(&config)?.len() <= crate::config::runtime::MAX_CONFIG_BYTES,
            "script result exceeds 8 MiB YAML"
        );
        crate::config::runtime::generate(config, &Mapping::new())
    })();
    let logs = logs.lock().unwrap().clone();
    match evaluated {
        Ok(config) => ScriptResponse {
            config: Some(config),
            logs,
            error: None,
        },
        Err(error) => ScriptResponse {
            config: None,
            logs,
            error: Some(format!("{error:#}").chars().take(4096).collect()),
        },
    }
}
