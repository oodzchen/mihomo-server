#![cfg(target_os = "linux")]
use anyhow::Result;
use headless_core::{config::runtime::parse, enhance::script::ScriptRequest};
use mihomo_server::script::execute;
use std::{path::PathBuf, time::Duration};
use tokio::sync::watch;
fn request(source: &str) -> Result<ScriptRequest> {
    Ok(ScriptRequest {
        check_only: false,
        source: source.into(),
        config: parse("mode: rule")?,
        name: "base".into(),
    })
}

#[tokio::test]
#[ignore = "requires isolated worker process/resource-limit verification"]
async fn real_worker_bounds_time_memory_logs_and_result_size() -> Result<()> {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server"));
    let (_cancel, mut shutdown) = watch::channel(false);
    let response = execute(
        &binary,
        request("function main(c) { c.mode='direct'; return c; }")?,
        &mut shutdown,
        Duration::from_secs(5),
    )
    .await?;
    assert_eq!(response.config.unwrap()["mode"].as_str(), Some("direct"));
    let response = execute(
        &binary,
        request("function main(c) { for(let i=0;i<1001;i++) console.info(i); return c; }")?,
        &mut shutdown,
        Duration::from_secs(5),
    )
    .await?;
    assert!(response.error.as_deref().unwrap().contains("output limit"));
    assert_eq!(response.logs.len(), 1000);
    let response = execute(
        &binary,
        request("function main(c) { c.big='a'.repeat(9*1024*1024); return c; }")?,
        &mut shutdown,
        Duration::from_secs(5),
    )
    .await?;
    assert!(response.error.as_deref().unwrap().contains("8 MiB"));
    let error = execute(
        &binary,
        request("function main(c) { while(true) {} }")?,
        &mut shutdown,
        Duration::from_millis(30),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("timed out"));
    assert!(
        execute(
            &binary,
            request("function main(c) { c.big='x'.repeat(800*1024*1024); return c; }")?,
            &mut shutdown,
            Duration::from_secs(5)
        )
        .await
        .is_err()
    );
    Ok(())
}
#[tokio::test]
#[ignore = "requires isolated worker shutdown/reaping verification"]
async fn cancelled_worker_is_killed_reaped_and_does_not_hold_pipe_tasks() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("ms-worker-{}-{stamp:x}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let result=async{
        let binary=directory.join("worker.py");let pid_path=directory.join("pid");
        std::fs::write(&binary,format!("#!/usr/bin/python3\nimport os,time,pathlib\npathlib.Path({:?}).write_text(str(os.getpid()))\nwhile True: time.sleep(1)\n",pid_path.to_str().unwrap()))?;std::fs::set_permissions(&binary,std::fs::Permissions::from_mode(0o700))?;
        let(cancel,mut shutdown)=watch::channel(false);let task=tokio::spawn(async move{execute(&binary,request("function main(c) { return c; }")?,&mut shutdown,Duration::from_secs(5)).await});
        tokio::time::timeout(Duration::from_secs(2),async{while !pid_path.is_file(){tokio::time::sleep(Duration::from_millis(10)).await;}}).await?;
        let pid=std::fs::read_to_string(pid_path)?.parse::<i32>()?;cancel.send_replace(true);assert!(format!("{:#}",task.await?.unwrap_err()).contains("cancelled"));
        // SAFETY: signal zero queries existence of the owned fixture PID only.
        assert_eq!(unsafe{libc::kill(pid,0)},-1);assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::ESRCH));
        Ok::<_,anyhow::Error>(())
    }.await;
    std::fs::remove_dir_all(directory)?;
    result
}
