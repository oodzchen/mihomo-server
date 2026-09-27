#![cfg(unix)]

use anyhow::{Context as _, Result};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions},
    management::{
        Management,
        auth::Authentication,
        http::{HttpState, MAX_REQUEST_BYTES, router},
    },
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    os::unix::fs::{PermissionsExt as _, symlink},
    path::PathBuf,
};
use tower::ServiceExt as _;

struct Directory(PathBuf);

#[tokio::test]
async fn core_release_commands_authenticate_reject_source_overrides_and_require_managed_resources() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let token = directory.token()?;
    let result = async {
        let prior = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        for command in [
            json!({"command":"core_release"}),
            json!({"command":"prepare_core_upgrade"}),
            json!({"command":"prepared_core_upgrade","id":"v1.2.3-invalid"}),
        ] {
            assert_eq!(
                response(&app, request("wrong", "/api/commands", Some(command))?)
                    .await?
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        for command in [
            json!({"command":"core_release","version":"../private"}),
            json!({"command":"core_release","url":"https://untrusted.invalid"}),
            json!({"command":"prepare_core_upgrade","sha256":"arbitrary"}),
            json!({"command":"prepare_core_upgrade","version":"v1.2.3"}),
            json!({"command":"prepared_core_upgrade","id":"v1.2.3-invalid"}),
            json!({"command":"prepared_core_upgrade","id":"v1.2.3-invalid","directory":"/tmp"}),
        ] {
            assert_eq!(
                response(&app, request(&token, "/api/commands", Some(command))?)
                    .await?
                    .0,
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        assert_eq!(serde_json::to_value(manager.profiles())?, prior);
        assert_eq!(manager.status().config_revision, revision);
        assert!(!directory.0.join(".upgrade-staging").exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn proxy_access_authenticates_and_distinguishes_saved_settings_from_stopped_config() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let token = directory.token()?;
    let result = async {
        let payload = json!({"command":"proxy_access"});
        assert_eq!(response(&app, request("wrong", "/api/commands", Some(payload.clone()))?).await?.0, StatusCode::UNAUTHORIZED);
        let (code, initial) = response(&app, request(&token, "/api/commands", Some(payload.clone()))?).await?;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(initial["has_config"], false);
        assert_eq!(initial["running"], false);
        assert!(initial["ports"][0]["actual"].is_null());
        let base = manager.import_profile_yaml("mixed-port: 12345\nport: 12346\nmode: direct\nauthentication: ['user:private-password']\nsecret: private-secret".into(), "test".into()).await?;
        manager.select_profile(base.uid.unwrap().to_string()).await?;
        manager.set_settings(serde_yaml_ng::from_str("mixed-port: 12347")?).await?;
        let (code, access) = response(&app, request(&token, "/api/commands", Some(payload))?).await?;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(access["has_config"], true);
        assert_eq!(access["ports"][0]["configured"], 12347);
        assert_eq!(access["ports"][0]["setting"], 12347);
        assert!(access["ports"][0]["actual"].is_null());
        assert_eq!(access["ports"][1]["configured"], 12346);
        assert!(access["ports"][1]["setting"].is_null());
        assert!(!access.to_string().contains("private-password"));
        assert!(!access.to_string().contains("private-secret"));
        assert_eq!(response(&app, request(&token, "/api/commands", Some(json!({"command":"proxy_access","unexpected":true})))?).await?.0, StatusCode::UNPROCESSABLE_ENTITY);
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn profile_dns_commands_authenticate_decode_strictly_and_do_not_confirm_inactive_profiles() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let token = directory.token()?;
    let result = async {
        let uid = manager
            .import_profile_yaml(
                "dns: {nameserver-policy: {example.org: 1.1.1.1}}".into(),
                "provider".into(),
            )
            .await?
            .uid
            .unwrap()
            .to_string();
        for command in [
            json!({"command":"profile_dns", "uid":uid}),
            json!({"command":"set_profile_dns", "uid":uid, "enabled":true}),
        ] {
            assert_eq!(
                response(&app, request("wrong", "/api/commands", Some(command))?)
                    .await?
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        for command in [
            json!({"command":"profile_dns", "uid":uid, "enabled":true}),
            json!({"command":"set_profile_dns", "uid":uid}),
            json!({"command":"set_profile_dns", "uid":uid, "enabled":"true"}),
            json!({"command":"set_profile_dns", "uid":uid, "enabled":true,"confirmation":[]}),
        ] {
            assert!(
                !response(&app, request(&token, "/api/commands", Some(command))?)
                    .await?
                    .0
                    .is_success()
            );
        }
        let (status, state) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"profile_dns", "uid":uid})),
            )?,
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(state["source"].as_str().unwrap().len(), 64);
        assert_eq!(state["requested"], false);
        assert_eq!(state["enabled"], false);
        manager
            .set_settings(serde_yaml_ng::from_str("dns: {nameserver: [1.1.1.1]}")?)
            .await?;
        let prior = manager.settings().await?;
        let (status, _) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"set_profile_dns", "uid":uid, "enabled":true,"confirmation":state["source"]})),
            )?,
        )
        .await?;
        assert!(!status.is_success());
        assert_eq!(manager.settings().await?, prior);
        assert!(prior.profile_dns.is_empty());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
async fn settings_commands_authenticate_validate_full_replacement_and_work_before_configuration() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let token = directory.token()?;
    let result = async {
        for command in [
            json!({"command":"settings"}),
            json!({"command":"set_settings","runtime":{"mode":"rule"}}),
        ] {
            assert_eq!(
                response(&app, request("wrong", "/api/commands", Some(command))?)
                    .await?
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        for command in [
            json!({"command":"settings","runtime":{}}),
            json!({"command":"set_settings"}),
            json!({"command":"set_settings","runtime":null}),
            json!({"command":"set_settings","runtime":{"mode":"invalid"}}),
            json!({"command":"set_settings","runtime":{"mixed-port":65536}}),
            json!({"command":"set_settings","runtime":{"external-controller":"127.0.0.1:9099"}}),
            json!({"command":"set_settings","runtime":{"dns":{"nameserver-policy":{}}}}),
            json!({"command":"set_settings","runtime":{"dns":{"enable":"true"}}}),
            json!({"command":"set_settings","runtime":{"tun":{"unknown":true}}}),
            json!({"command":"set_settings","runtime":{"tun":{"mtu":0}}}),
            json!({"command":"set_settings","runtime":{},"schema_version":2}),
        ] {
            assert!(
                !response(&app, request(&token, "/api/commands", Some(command))?)
                    .await?
                    .0
                    .is_success()
            );
            assert!(manager.settings().await?.runtime.mode.is_none());
        }
        let initial = manager.status().config_revision;
        let (status, settings) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"set_settings","runtime":{"mode":"global","mixed-port":0,"allow-lan":false}})),
            )?,
        )
        .await?;
        assert!(status.is_success());
        assert_eq!(settings["schema_version"], 1);
        assert_eq!(settings["runtime"]["mode"], "global");
        assert_eq!(settings["runtime"]["allow-lan"], false);
        assert_eq!(manager.status().config_revision, initial);
        let (_, saved) = response(
            &app,
            request(&token, "/api/commands", Some(json!({"command":"settings"})))?,
        )
        .await?;
        assert_eq!(saved, settings);
        let runtime =
            json!({"dns":{"enable":false,"nameserver":["1.1.1.1"]},"tun":{"enable":false,"dns-hijack":[],"mtu":1500}});
        let (status, nested) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"set_settings","runtime":runtime})),
            )?,
        )
        .await?;
        assert!(status.is_success());
        assert_eq!(nested["runtime"], runtime);
        assert_eq!(manager.status().config_revision, initial);
        let (_, saved) = response(
            &app,
            request(&token, "/api/commands", Some(json!({"command":"settings"})))?,
        )
        .await?;
        assert_eq!(saved, nested);
        let (status, cleared) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"set_settings","runtime":{}})),
            )?,
        )
        .await?;
        assert!(status.is_success());
        assert_eq!(cleared["runtime"], json!({}));
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result?;
    cleanup
}
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = Self(std::env::temp_dir().join(format!("ms-http-{}-{stamp:x}", std::process::id())));
        std::fs::create_dir(&directory.0)?;
        Ok(directory)
    }
    fn authentication(&self) -> Result<Authentication> {
        Authentication::load_or_create(&self.0.join("management-token"), "127.0.0.1:9090".parse()?, None)
    }
    fn token(&self) -> Result<String> {
        Ok(std::fs::read_to_string(self.0.join("management-token"))?
            .trim()
            .to_owned())
    }
    fn manager(&self) -> Result<CoreManager> {
        let validator = self.0.join("validator.py");
        std::fs::write(
            &validator,
            "#!/usr/bin/python3\nimport sys\nsys.exit(0 if '-t' in sys.argv else 1)\n",
        )?;
        std::fs::set_permissions(&validator, std::fs::Permissions::from_mode(0o700))?;
        let mut options = CoreOptions::new(validator, self.0.clone(), self.0.join("missing.yaml"));
        options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
        CoreManager::spawn(options)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request(token: &str, path: &str, command: Option<Value>) -> Result<Request<Body>> {
    Ok(Request::builder()
        .method(if command.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header(header::HOST, "127.0.0.1:9090")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(command.map(|value| value.to_string()).unwrap_or_default()))?)
}
async fn response(app: &Router, request: Request<Body>) -> Result<(StatusCode, Value)> {
    let response = app.clone().oneshot(request).await?;
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let status = response.status();
    Ok((
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), MAX_REQUEST_BYTES).await?)?,
    ))
}

#[tokio::test]
async fn authenticated_metadata_edit_and_delete_reject_ownership_fields_and_protect_active_uid() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let token = directory.token()?;
    let result =
        async {
            let first = manager
                .import_profile_yaml("mode: direct".into(), "current".into())
                .await?;
            let second = manager
                .import_profile_yaml("mode: rule".into(), "editable".into())
                .await?;
            let first_uid = first.uid.as_deref().unwrap();
            let second_uid = second.uid.as_deref().unwrap();
            manager.select_profile(first_uid.into()).await?;
            let prior = serde_json::to_value(manager.profiles())?;
            let revision = manager.status().config_revision;
            for command in [
                json!({"command":"edit_profile", "uid":second_uid,"patch":{"name":"unauthorized"}}),
                json!({"command":"delete_profile", "uid":second_uid}),
            ] {
                assert_eq!(
                    response(&app, request("bad-token", "/api/commands", Some(command))?)
                        .await?
                        .0,
                    StatusCode::UNAUTHORIZED
                );
            }
            for patch in [
                json!({"uid":"new"}),
                json!({"file":"../outside.yaml"}),
                json!({"type":"script"}),
                json!({"selected":[]}),
                json!({"extra":{}}),
                json!({"updated":1}),
                json!({"options":{"with_proxy":true}}),
                json!({"options":{"script":"injected"}}),
                json!({"options":{"unknown":true}}),
                json!({"options":{"timeout_seconds":-1}}),
                json!({"name":" "}),
                json!({"url":"https://example.test/sub"}),
            ] {
                assert!(
                    !response(
                        &app,
                        request(
                            &token,
                            "/api/commands",
                            Some(json!({
                                "command":"edit_profile","uid":second_uid,"patch":patch
                            }))
                        )?
                    )
                    .await?
                    .0
                    .is_success()
                );
                assert_eq!(serde_json::to_value(manager.profiles())?, prior);
            }
            for command in [
                json!({"command":"delete_profile","uid":first_uid}),
                json!({"command":"delete_profile","uid":"missing"}),
                json!({"command":"delete_profile","uid":second_uid,"force":true}),
            ] {
                assert!(
                    !response(&app, request(&token, "/api/commands", Some(command))?)
                        .await?
                        .0
                        .is_success()
                );
            }
            let (status, item) = response(&app, request(&token,"/api/commands",Some(json!({
            "command":"edit_profile", "uid":second_uid, "patch":{"name":"renamed", "desc":"saved description"}
        })))?).await?;
            assert!(status.is_success());
            assert_eq!(item["uid"], second_uid);
            assert_eq!(item["name"], "renamed");
            assert_eq!(item["file"], second.file.as_deref().unwrap());
            assert_eq!(manager.status().config_revision, revision);
            let (status, catalog) = response(
                &app,
                request(
                    &token,
                    "/api/commands",
                    Some(json!({
                        "command":"delete_profile", "uid":second_uid
                    })),
                )?,
            )
            .await?;
            assert!(status.is_success());
            assert_eq!(catalog["items"].as_array().unwrap().len(), 8);
            assert_eq!(catalog["current"], first_uid);
            assert_eq!(manager.status().config_revision, revision);
            assert!(
                !directory
                    .0
                    .join("profiles")
                    .join(second.file.as_deref().unwrap())
                    .exists()
            );
            assert!(
                directory
                    .0
                    .join("profiles")
                    .join(first.file.as_deref().unwrap())
                    .is_file()
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn authenticated_merge_commands_keep_raw_content_and_reject_implicit_clear_or_controller_escape() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let token = directory.token()?;
    let result = async {
        let base = manager.import_profile_yaml("mode: rule".into(), "base".into()).await?;
        let uid = base.uid.as_deref().unwrap();
        for command in ["profile_merge", "set_profile_merge", "clear_profile_merge"] {
            let payload = if command == "set_profile_merge" {
                json!({"command":command,"uid":uid,"yaml":"mode: direct"})
            } else {
                json!({"command":command,"uid":uid})
            };
            assert_eq!(
                response(&app, request("wrong", "/api/commands", Some(payload))?)
                    .await?
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        let (_, content) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"profile_merge","uid":uid})),
            )?,
        )
        .await?;
        assert_eq!(content["uid"].as_str(), base.option.as_ref().unwrap().merge.as_deref());
        for payload in [
            json!({"command":"set_profile_merge","uid":uid}),
            json!({"command":"set_profile_merge","uid":uid,"yaml":null}),
            json!({"command":"set_profile_merge","uid":uid,"yaml":"mode: direct","file":"outside"}),
            json!({"command":"set_profile_merge","uid":uid,"yaml":"- list"}),
            json!({"command":"set_profile_merge","uid":uid,"yaml":"external-controller: '127.0.0.1:9999'"}),
        ] {
            assert!(
                !response(&app, request(&token, "/api/commands", Some(payload))?)
                    .await?
                    .0
                    .is_success()
            );
            assert_eq!(
                manager.profile_merge(uid.into()).await?.uid.as_deref(),
                base.option.as_ref().unwrap().merge.as_deref()
            );
        }
        let (status, item) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({
                    "command":"set_profile_merge","uid":uid,"yaml":"# saved merge\nmode: direct"
                })),
            )?,
        )
        .await?;
        assert!(status.is_success());
        assert_eq!(item["uid"], uid);
        assert_eq!(item["file"], base.file.as_deref().unwrap());
        assert_eq!(manager.profiles().items.unwrap().len(), 8);
        assert!(manager.status().config_revision.is_none());
        let (_, content) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"profile_merge","uid":uid})),
            )?,
        )
        .await?;
        assert_eq!(content["yaml"], "# saved merge\nmode: direct");
        manager.select_profile(uid.into()).await?;
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("direct"));
        let (status, item) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"clear_profile_merge","uid":uid})),
            )?,
        )
        .await?;
        assert!(status.is_success());
        assert!(item["option"]["merge"].is_null());
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("rule"));
        assert_eq!(
            std::fs::read_to_string(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
            "mode: rule"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn sequence_commands_authenticate_strict_types_preserve_raw_and_clear_explicitly() -> Result<()> {
    use headless_core::config::profile_store::SequenceKind;
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let token = directory.token()?;
    let result = async {
        let raw = "proxies: []\nrules: ['MATCH,DIRECT']\n";
        let base = manager.import_profile_yaml(raw.into(), "base".into()).await?;
        let uid = base.uid.as_deref().unwrap();
        let yaml = "prepend: ['DOMAIN,sequence.test,REJECT']\nappend: []\ndelete: []\n";
        for command in ["profile_sequence", "set_profile_sequence", "clear_profile_sequence"] {
            let mut payload = json!({"command": command, "uid": uid, "kind": "rules"});
            if command == "set_profile_sequence" {
                payload["yaml"] = yaml.into();
            }
            assert_eq!(
                response(&app, request("wrong", "/api/commands", Some(payload))?)
                    .await?
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        for payload in [
            json!({"command":"set_profile_sequence","uid":uid,"kind":"merge","yaml":yaml}),
            json!({"command":"set_profile_sequence","uid":uid,"yaml":yaml}),
            json!({"command":"set_profile_sequence","uid":uid,"kind":"rules"}),
            json!({"command":"set_profile_sequence","uid":uid,"kind":"rules","yaml":null}),
            json!({"command":"set_profile_sequence","uid":uid,"kind":"rules","yaml":yaml,"file":"outside"}),
            json!({"command":"set_profile_sequence","uid":uid,"kind":"rules","yaml":"prepend: []"}),
            json!({"command":"set_profile_sequence","uid":uid,"kind":"proxies","yaml":yaml}),
        ] {
            assert!(
                !response(&app, request(&token, "/api/commands", Some(payload))?)
                    .await?
                    .0
                    .is_success()
            );
        }
        let (status, item) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"set_profile_sequence","uid":uid,"kind":"rules","yaml":yaml})),
            )?,
        )
        .await?;
        assert!(status.is_success());
        assert_eq!(item["file"], base.file.as_deref().unwrap());
        assert!(manager.status().config_revision.is_none());
        let (_, content) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"profile_sequence","uid":uid,"kind":"rules"})),
            )?,
        )
        .await?;
        assert_eq!(content["yaml"], yaml);
        assert!(content["uid"].as_str().unwrap().starts_with('r'));
        manager.select_profile(uid.into()).await?;
        assert_eq!(
            manager.runtime_config().await?["rules"][0].as_str(),
            Some("DOMAIN,sequence.test,REJECT")
        );
        let (status, _) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"clear_profile_sequence","uid":uid,"kind":"rules"})),
            )?,
        )
        .await?;
        assert!(status.is_success());
        assert_eq!(
            manager.runtime_config().await?["rules"][0].as_str(),
            Some("MATCH,DIRECT")
        );
        assert!(
            manager
                .profile_sequence(uid.into(), SequenceKind::Rules)
                .await?
                .uid
                .is_none()
        );
        assert_eq!(manager.profiles().items.unwrap().len(), 7);
        assert_eq!(
            std::fs::read_to_string(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
            raw
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn script_commands_are_authenticated_strict_and_keep_raw_when_execution_fails() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let token = directory.token()?;
    let result=async{
        let base=manager.import_profile_yaml("mode: rule".into(),"base".into()).await?;let uid=base.uid.as_deref().unwrap();let source="function main(c,name) { console.info(name); c.mode='direct'; return c; }";
        for command in ["profile_script","set_profile_script","clear_profile_script"] {
            let mut payload=json!({"command":command,"uid":uid});if command=="set_profile_script"{payload["source"]=source.into();}
            assert_eq!(response(&app,request("wrong","/api/commands",Some(payload))?).await?.0,StatusCode::UNAUTHORIZED);
        }
        for payload in [json!({"command":"set_profile_script","uid":uid}),json!({"command":"set_profile_script","uid":uid,"source":null}),json!({"command":"set_profile_script","uid":uid,"source":source,"file":"outside"}),json!({"command":"set_profile_script","uid":uid,"source":"function main(c) { throw 'failed'; }"}),json!({"command":"set_profile_script","uid":uid,"source":"function main(c) { c['external-controller']='0.0.0.0:9999'; return c; }"})] {
            assert!(!response(&app,request(&token,"/api/commands",Some(payload))?).await?.0.is_success());assert_eq!(manager.profile_script(uid.into()).await?.uid.as_deref(),base.option.as_ref().unwrap().script.as_deref());
        }
        let (status,item)=response(&app,request(&token,"/api/commands",Some(json!({"command":"set_profile_script","uid":uid,"source":source})))?).await?;
        assert!(status.is_success(),"{item}");assert_eq!(item["file"],base.file.as_deref().unwrap());assert!(manager.status().config_revision.is_none());
        let (_,content)=response(&app,request(&token,"/api/commands",Some(json!({"command":"profile_script","uid":uid})))?).await?;assert_eq!(content["source"],source);
        manager.select_profile(uid.into()).await?;assert_eq!(manager.runtime_config().await?["mode"].as_str(),Some("direct"));
        let revision=manager.status().config_revision;
        assert!(manager.set_profile_script(uid.into(),Some("function main(c) { console.warn('before error'); throw 'failed'; }".into())).await.is_err());
        assert_eq!(manager.status().config_revision,revision);assert!(manager.logs().iter().any(|log|log.stream=="script"&&log.message.contains("before error")));
        let(status,_)=response(&app,request(&token,"/api/commands",Some(json!({"command":"clear_profile_script","uid":uid})))?).await?;assert!(status.is_success());
        assert_eq!(manager.runtime_config().await?["mode"].as_str(),Some("rule"));assert_eq!(manager.profiles().items.unwrap().len(),7);
        assert_eq!(std::fs::read_to_string(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,"mode: rule");
        Ok::<_,anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[test]
fn authentication_persists_private_credentials_and_requires_exact_origin() -> Result<()> {
    let directory = Directory::new()?;
    let authentication = directory.authentication()?;
    let token = directory.token()?;
    assert_eq!(token.len(), 64);
    assert_eq!(
        std::fs::metadata(directory.0.join("management-token"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    directory.authentication()?;
    assert_eq!(directory.token()?, token);
    let bearer = format!("Bearer {token}");
    assert!(
        authentication
            .authorize("127.0.0.1:9090", Some("http://127.0.0.1:9090"), Some(&bearer))
            .is_ok()
    );
    assert!(authentication.authorize("127.0.0.1:9090", None, Some(&bearer)).is_ok());
    for supplied in ["", "Bearer bad", token.as_str(), "Basic bad"] {
        assert!(
            authentication
                .authorize("127.0.0.1:9090", None, Some(supplied))
                .is_err()
        );
    }
    for origin in ["null", "http://evil.test", "http://127.0.0.1:9090/"] {
        assert!(
            authentication
                .authorize("127.0.0.1:9090", Some(origin), Some(&bearer))
                .is_err()
        );
    }
    assert!(authentication.authorize("evil.test", None, Some(&bearer)).is_err());
    Ok(())
}

#[test]
fn authentication_rejects_unsafe_files_and_invalid_listener_settings() -> Result<()> {
    let directory = Directory::new()?;
    let path = directory.0.join("management-token");
    let listen: SocketAddr = "0.0.0.0:9090".parse()?;
    assert!(Authentication::load_or_create(&path, listen, None).is_err());
    for origin in [
        "ftp://example.test",
        "https://a:b@example.test",
        "https://example.test/path",
        "https://example.test/?x=y",
        "https://example.test/#hash",
    ] {
        assert!(Authentication::load_or_create(&path, listen, Some(origin)).is_err());
    }
    assert!(Authentication::load_or_create(&path, "127.0.0.1:0".parse()?, None).is_err());
    let authentication = Authentication::load_or_create(&path, listen, Some("https://example.test:443"))?;
    assert_eq!(authentication.origin(), "https://example.test");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
    assert!(directory.authentication().is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&path, directory.0.join("original"))?;
    symlink(directory.0.join("original"), &path)?;
    assert!(directory.authentication().is_err());
    std::fs::remove_file(&path)?;
    std::fs::write(&path, "invalid")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    assert!(directory.authentication().is_err());
    Ok(())
}

#[tokio::test]
async fn http_authentication_precedes_body_parsing_and_rejects_duplicate_headers() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let result = async {
        let token = directory.token()?;
        for name in [header::HOST, header::ORIGIN, header::AUTHORIZATION] {
            let mut request = request(&token, "/api/status", None)?;
            request.headers_mut().append(name.clone(), "bad".parse()?);
            if name == header::ORIGIN {
                request.headers_mut().append(name, "bad".parse()?);
            }
            assert_eq!(response(&app, request).await?.0, StatusCode::BAD_REQUEST);
        }
        let mut missing = request(&token, "/api/commands", Some(json!({"command":"stop"})))?;
        missing.headers_mut().remove(header::AUTHORIZATION);
        *missing.body_mut() = Body::from(vec![b'x'; MAX_REQUEST_BYTES + 1]);
        assert_eq!(response(&app, missing).await?.0, StatusCode::UNAUTHORIZED);
        for (name, value) in [(header::HOST, "evil.test"), (header::ORIGIN, "http://evil.test")] {
            let mut request = request(&token, "/api/status", None)?;
            request.headers_mut().insert(name, value.parse()?);
            assert_eq!(response(&app, request).await?.0, StatusCode::UNAUTHORIZED);
        }
        assert_eq!(
            response(&app, request(&token, "/api/status?token=bad", None)?).await?.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Stopped);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn http_errors_limits_and_shutdown_are_structured() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let state = HttpState::new(Management::new(manager.clone(), directory.authentication()?));
    let app = router(state.clone());
    let result = async {
        let token = directory.token()?;
        assert_eq!(
            response(&app, request(&token, "/api/absent", None)?).await?.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            response(&app, request(&token, "/api/commands", None)?).await?.0,
            StatusCode::METHOD_NOT_ALLOWED
        );
        for command in [
            json!({"command":"shell", "path":"/tmp/arbitrary"}),
            json!({"command":"stop", "unexpected":true}),
        ] {
            let (status, value) = response(&app, request(&token, "/api/commands", Some(command))?).await?;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(value["error"]["code"], "invalid_request");
        }
        let mut oversized = request(&token, "/api/commands", Some(json!({"command":"status"})))?;
        *oversized.body_mut() = Body::from(vec![b'x'; MAX_REQUEST_BYTES + 1]);
        assert_eq!(response(&app, oversized).await?.0, StatusCode::PAYLOAD_TOO_LARGE);
        state.close();
        assert_eq!(
            response(&app, request(&token, "/api/status", None)?).await?.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn http_import_select_config_and_failed_start_share_the_manager() -> Result<()> {
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let result = async {
        let token = directory.token()?;
        let (status, _) = response(
            &app,
            request(&token, "/api/commands", Some(json!({"command":"start"})))?,
        )
        .await?;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response(&app, request(&token, "/api/status", None)?).await?.1["phase"],
            "failed"
        );
        let (status, item) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"import_profile", "name":"Uploaded", "yaml":"mode: rule\n"})),
            )?,
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let uid = item["uid"].as_str().context("missing imported UID")?;
        let (status, _) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"select_profile", "uid":uid})),
            )?,
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response(&app, request(&token, "/api/profiles", None)?).await?.1["current"],
            uid
        );
        let edited = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"edit_config", "yaml":"mode: direct\n"})),
            )?,
        )
        .await?;
        assert_eq!(edited.0, StatusCode::OK);
        assert_eq!(manager.status().active_profile.as_deref(), Some(uid));
        assert_eq!(manager.profiles().current.as_deref(), Some(uid));
        let committed = manager.status().config_revision;
        let (status, value) = response(&app, request(&token, "/api/config", None)?).await?;
        assert_eq!(status, StatusCode::OK);
        assert!(value["yaml"].as_str().context("missing YAML")?.contains("mode: direct"));
        for command in [
            json!({"command":"apply_config", "yaml":"[invalid]"}),
            json!({"command":"edit_config", "yaml":"external-controller: '0.0.0.0:9000'"}),
            json!({"command":"import_profile", "name":"", "yaml":"mode: direct"}),
            json!({"command":"apply_overlay", "yaml":"external-controller: '0.0.0.0:9000'"}),
        ] {
            assert_eq!(
                response(&app, request(&token, "/api/commands", Some(command))?)
                    .await?
                    .0,
                StatusCode::UNPROCESSABLE_ENTITY
            );
            assert_eq!(manager.status().config_revision, committed);
            assert_eq!(manager.status().active_profile.as_deref(), Some(uid));
        }
        assert_eq!(manager.profiles().items.context("missing profiles")?.len(), 8);
        assert_eq!(
            response(&app, request(&token, "/api/proxies", None)?).await?.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let standalone = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"apply_config", "yaml":"mode: rule\n"})),
            )?,
        )
        .await?;
        assert_eq!(standalone.0, StatusCode::OK);
        assert!(manager.status().active_profile.is_none());
        assert!(manager.profiles().current.is_none());
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires a real core and TCP/Unix socket binding permissions"]
async fn live_http_repairs_failed_start_and_restores_selection_after_service_restart() -> Result<()> {
    use futures_util::{SinkExt as _, StreamExt as _};
    use std::{process::Stdio, time::Duration};
    use tokio::{
        process::Command,
        time::{sleep, timeout},
    };
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let directory = Directory::new()?;
    let binary = std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?;
    let address = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?;
    let base = format!("http://{address}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(8))
        .build()?;
    let mut uid = None;
    for initial in [true, false] {
        let mut service = Command::new(env!("CARGO_BIN_EXE_mihomo-server"))
            .arg("--mihomo")
            .arg(&binary)
            .arg("--data-dir")
            .arg(&directory.0)
            .arg("--listen")
            .arg(address.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let service_pid = service.id().context("service PID missing")?;
        let mut core_pid = None::<u32>;
        let mut active_sockets = Vec::new();
        let result = async {
            let token = timeout(Duration::from_secs(8), async {
                loop {
                    if let Ok(token) = directory.token() { break Ok::<_, anyhow::Error>(token); }
                    anyhow::ensure!(service.try_wait()?.is_none(), "service exited before token creation");
                    sleep(Duration::from_millis(20)).await;
                }
            }).await??;
            let status = timeout(Duration::from_secs(8), async {
                loop {
                    if let Ok(response) = client.get(format!("{base}/api/status")).bearer_auth(&token).send().await {
                        let state: Value = response.error_for_status()?.json().await?;
                        if state["phase"] == if initial { "failed" } else { "running" } {
                            break Ok::<_, anyhow::Error>(state);
                        }
                    }
                    anyhow::ensure!(service.try_wait()?.is_none(), "service exited before readiness");
                    sleep(Duration::from_millis(20)).await;
                }
            }).await??;
            assert_eq!(client.get(format!("{base}/api/status")).send().await?.status(), StatusCode::UNAUTHORIZED);
            let command = |value: Value| client.post(format!("{base}/api/commands"))
                .bearer_auth(&token).header(header::ORIGIN, &base).json(&value);
            if initial {
                assert!(status["error"].is_string());
                let yaml = "mixed-port: 0\nmode: rule\nexternal-controller: ''\ndns: {enable: false}\ntun: {enable: false}\nprofile: {store-selected: false}\nproxy-groups:\n  - {name: Main, type: select, proxies: [DIRECT, REJECT]}\nrules: ['MATCH,Main']\n";
                let item: Value = command(json!({"command":"import_profile", "name":"HTTP import", "yaml":yaml}))
                    .send().await?.error_for_status()?.json().await?;
                uid = Some(item["uid"].as_str().context("missing UID")?.to_owned());
                command(json!({"command":"select_profile", "uid":uid})).send().await?.error_for_status()?;
                let started: Value = command(json!({"command":"start"})).send().await?.error_for_status()?.json().await?;
                core_pid = Some(started["pid"].as_u64().context("missing core PID")? as u32);
                command(json!({"command":"select_node", "group":"Main", "node":"REJECT"})).send().await?.error_for_status()?;
                let revision = started["config_revision"].clone();
                assert_eq!(command(json!({"command":"apply_overlay", "yaml":"rules: [INVALID,DIRECT]"})).send().await?.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let status: Value = client.get(format!("{base}/api/status")).bearer_auth(&token).send().await?.error_for_status()?.json().await?;
                assert_eq!(status["config_revision"], revision);
                assert_eq!(status["phase"], "running");
                command(json!({"command":"apply_overlay", "yaml":"log-level: debug"})).send().await?.error_for_status()?;
                command(json!({"command":"stop"})).send().await?.error_for_status()?;
                let state: Value = client.get(format!("{base}/api/status")).bearer_auth(&token).send().await?.error_for_status()?.json().await?;
                assert_eq!(state["phase"], "stopped");
                assert_eq!(client.get(format!("{base}/api/proxies")).bearer_auth(&token).send().await?.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let restarted: Value = command(json!({"command":"restart"})).send().await?.error_for_status()?.json().await?;
                core_pid = Some(restarted["pid"].as_u64().context("missing restarted PID")? as u32);
            } else {
                core_pid = Some(status["pid"].as_u64().context("missing restored PID")? as u32);
                assert_eq!(status["active_profile"].as_str(), uid.as_deref());
            }
            timeout(Duration::from_secs(5), async {
                loop {
                    let proxies: Value = client.get(format!("{base}/api/proxies")).bearer_auth(&token)
                        .send().await?.error_for_status()?.json().await?;
                    if proxies["proxies"]["Main"]["now"] == "REJECT" { break Ok::<_, anyhow::Error>(()); }
                    sleep(Duration::from_millis(20)).await;
                }
            }).await??;
            let profiles: Value = client.get(format!("{base}/api/profiles")).bearer_auth(&token)
                .send().await?.error_for_status()?.json().await?;
            assert_eq!(profiles["current"].as_str(), uid.as_deref());
            assert_eq!(profiles["items"].as_array().unwrap().iter().find(|item| item["uid"].as_str() == uid.as_deref()).unwrap()["selected"][0]["now"], "REJECT");
            let logs: Value = client.get(format!("{base}/api/logs")).bearer_auth(&token)
                .send().await?.error_for_status()?.json().await?;
            assert!(!logs.as_array().context("invalid logs")?.is_empty());
            // Keep authenticated and pending upgrades open across real SIGTERM.
            for path in ["/api/events", "/api/streams/traffic"] {
                let (mut socket, _) = connect_async(format!("ws://{address}{path}")).await?;
                socket.send(Message::Text(json!({"type":"authenticate","token":token}).to_string().into())).await?;
                let message = timeout(Duration::from_secs(3), socket.next()).await?
                    .context("WebSocket ended before authentication")??;
                let Message::Text(text) = message else { anyhow::bail!("expected WebSocket ready frame"); };
                assert_eq!(serde_json::from_str::<Value>(&text)?["type"], "ready");
                active_sockets.push(socket);
            }
            active_sockets.push(connect_async(format!("ws://{address}/api/events")).await?.0);
            Ok::<(), anyhow::Error>(())
        }.await;
        // Cleanup is attempted even if a request/assertion fails.
        let cleanup = async {
            anyhow::ensure!(
                unsafe { libc::kill(service_pid as i32, libc::SIGTERM) } == 0,
                "service signal failed"
            );
            let exit = timeout(Duration::from_secs(12), service.wait()).await??;
            anyhow::ensure!(exit.success(), "service shutdown failed: {exit}");
            for socket in &mut active_sockets {
                timeout(Duration::from_secs(3), async {
                    loop {
                        match socket.next().await {
                            None | Some(Ok(Message::Close(_))) => break Ok::<(), anyhow::Error>(()),
                            Some(Err(error)) => return Err(error.into()),
                            _ => {}
                        }
                    }
                })
                .await??;
            }
            if let Some(pid) = core_pid {
                anyhow::ensure!(
                    unsafe { libc::kill(pid as i32, 0) } == -1
                        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
                    "owned core remains alive"
                );
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        result?;
        cleanup?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn authenticated_global_edits_are_strict_resettable_and_preserve_standalone_runtime() -> Result<()> {
    use headless_core::config::{
        profile_store::{DEFAULT_GLOBAL_MERGE, DEFAULT_GLOBAL_SCRIPT},
        runtime,
    };
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let app = router(HttpState::new(Management::new(
        manager.clone(),
        directory.authentication()?,
    )));
    let token = directory.token()?;
    let result = async {
        manager.apply_config(runtime::parse("mode: direct")?).await?;
        let revision = manager.status().config_revision;
        let initial = serde_json::to_value(manager.profiles())?;
        for command in [
            json!({"command":"global_merge"}), json!({"command":"global_script"}),
            json!({"command":"set_global_merge","yaml":"mode: global"}),
            json!({"command":"set_global_script","source":"function main(c) { return c; }"}),
            json!({"command":"reset_global_merge"}), json!({"command":"reset_global_script"}),
        ] {
            let (status, _) = response(&app, request("incorrect", "/api/commands", Some(command))?).await?;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(serde_json::to_value(manager.profiles())?, initial);
        }
        for command in [
            json!({"command":"global_merge","uid":"Merge"}),
            json!({"command":"global_script","path":"Script.js"}),
            json!({"command":"set_global_merge"}), json!({"command":"set_global_merge","yaml":null}),
            json!({"command":"set_global_merge","yaml":"- not a mapping"}),
            json!({"command":"set_global_merge","yaml":"EXTERNAL-CONTROLLER: '0.0.0.0:9090'"}),
            json!({"command":"set_global_script"}), json!({"command":"set_global_script","source":null}),
            json!({"command":"set_global_script","source":" "}),
            json!({"command":"set_global_script","source":"function main( {"}),
            json!({"command":"reset_global_script","source":"ignored"}),
        ] {
            let (status, _) = response(&app, request(&token, "/api/commands", Some(command.clone()))?).await?;
            assert!(!status.is_success(), "accepted {command}");
            assert_eq!(serde_json::to_value(manager.profiles())?, initial);
            assert_eq!(manager.status().config_revision, revision);
        }
        let (_, merge) = response(&app, request(&token, "/api/commands", Some(json!({"command":"global_merge"})))?).await?;
        assert_eq!(merge["uid"], "Merge"); assert_eq!(merge["yaml"], DEFAULT_GLOBAL_MERGE);
        let (_, script) = response(&app, request(&token, "/api/commands", Some(json!({"command":"global_script"})))?).await?;
        assert_eq!(script["uid"], "Script"); assert_eq!(script["source"], DEFAULT_GLOBAL_SCRIPT);
        let source = "function main(c) { throw Error('deferred execution'); }";
        for command in [json!({"command":"set_global_merge","yaml":"# saved\nmode: global"}), json!({"command":"set_global_script","source":source})] {
            let (status, row) = response(&app, request(&token, "/api/commands", Some(command))?).await?;
            assert!(status.is_success(), "{row}");
            assert!(matches!(row["uid"].as_str(), Some("Merge" | "Script")));
            assert!(manager.status().active_profile.is_none());
            assert_eq!(manager.status().config_revision, revision);
            assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("direct"));
        }
        let (_, saved) = response(&app, request(&token, "/api/commands", Some(json!({"command":"global_script"})))?).await?;
        assert_eq!(saved["source"], source);
        for command in ["reset_global_merge", "reset_global_script"] {
            let (status, row) = response(&app, request(&token, "/api/commands", Some(json!({"command":command})))?).await?;
            assert!(status.is_success(), "{row}");
        }
        assert_eq!(manager.global_merge().await?.yaml.as_deref(), Some(DEFAULT_GLOBAL_MERGE));
        assert_eq!(manager.global_script().await?.source.as_deref(), Some(DEFAULT_GLOBAL_SCRIPT));
        assert_eq!(manager.profiles().items.unwrap().len(), 2);
        let base = manager.import_profile_yaml("mode: rule".into(), "current".into()).await?;
        manager.select_profile(base.uid.unwrap().to_string()).await?;
        let current = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let (status, _) = response(&app, request(&token, "/api/commands", Some(json!({"command":"set_global_script","source":"function main(c) { console.warn('global rollback'); throw 'bad'; }"})))?).await?;
        assert!(!status.is_success());
        assert_eq!(serde_json::to_value(manager.profiles())?, current);
        assert_eq!(manager.status().config_revision, revision);
        assert!(manager.logs().iter().any(|log| log.stream == "script" && log.message.contains("global rollback")));
        assert!(!directory.0.join("profile-merge.yaml").exists());
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn raw_profile_commands_authenticate_reject_unknown_fields_and_stale_versions() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager()?;
    let app = router(HttpState::new(Management::new(manager.clone(), dir.authentication()?)));
    let token = dir.token()?;
    let result = async {
        let item = manager
            .import_profile_yaml("# raw\nmode: direct".into(), "raw".into())
            .await?;
        let uid = item.uid.unwrap().to_string();
        for command in [
            json!({"command":"profile_raw","uid":uid}),
            json!({"command":"set_profile_raw","uid":uid,"revision":"old","yaml":"mode: direct"}),
        ] {
            assert_eq!(
                response(&app, request("wrong", "/api/commands", Some(command))?)
                    .await?
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        for command in [
            json!({"command":"profile_raw","uid":uid,"file":"arbitrary"}),
            json!({"command":"set_profile_raw","uid":uid,"yaml":"mode: direct"}),
            json!({"command":"set_profile_raw","uid":uid,"revision":[],"yaml":"mode: direct"}),
            json!({"command":"set_profile_raw","uid":uid,"revision":"old","yaml":"mode: direct","extra":true}),
        ] {
            assert!(
                !response(&app, request(&token, "/api/commands", Some(command))?)
                    .await?
                    .0
                    .is_success()
            );
        }
        let (status, content) = response(
            &app,
            request(
                &token,
                "/api/commands",
                Some(json!({"command":"profile_raw","uid":uid})),
            )?,
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content["yaml"], "# raw\nmode: direct");
        assert_eq!(content["uid"], uid);
        for command in [
            json!({"command":"set_profile_raw","uid":uid,"revision":"old","yaml":"mode: rule"}),
            json!({"command":"set_profile_raw","uid":uid,"revision":content["revision"],"yaml":"- sequence"}),
            json!({"command":"profile_raw","uid":"Merge"}),
            json!({"command":"profile_raw","uid":"../outside"}),
        ] {
            assert!(
                !response(&app, request(&token, "/api/commands", Some(command))?)
                    .await?
                    .0
                    .is_success()
            );
        }
        assert_eq!(manager.profile_raw(uid).await?.yaml, "# raw\nmode: direct");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result?;
    cleanup
}
