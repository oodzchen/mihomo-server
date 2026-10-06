//! Ephemeral private certificate and owned HTTPS tasks; no checked-in keys or external network.
use anyhow::{Result, ensure};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};
pub const YAML: &str = "proxies: []\nmode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']\n";
pub struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> Result<Self> {
        use std::os::unix::fs::PermissionsExt as _;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-tls-{}-{stamp:x}", std::process::id()));
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        Ok(Self(path))
    }
    pub fn validator(&self) -> Result<PathBuf> {
        use std::os::unix::fs::PermissionsExt as _;
        let path = self.0.join("validator.py");
        fs::write(
            &path,
            "#!/usr/bin/env python3\nimport sys\nsys.exit(0 if '-t' in sys.argv else 1)\n",
        )?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        Ok(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub struct State {
    pub connections: AtomicUsize,
    pub requests: Mutex<Vec<String>>,
    pub handshake_delay_ms: AtomicU64,
    pub hold: AtomicBool,
    pub release: Semaphore,
    pub response: Mutex<String>,
}
impl State {
    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    pub async fn wait(&self, count: usize) -> Result<()> {
        timeout(Duration::from_secs(5), async {
            while self.count() < count {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(())
    }
}
pub struct Fixture {
    pub url: String,
    pub state: Arc<State>,
    task: JoinHandle<()>,
    _certificate: Directory,
}
impl Fixture {
    pub async fn new() -> Result<Self> {
        Self::with_subject("DNS:fixture.invalid").await
    }
    #[allow(dead_code)] // Used by the process-isolated platform-root fixture.
    pub async fn trusted_name() -> Result<Self> {
        Self::with_subject("IP:127.0.0.1").await
    }
    #[allow(dead_code)]
    pub fn certificate(&self) -> PathBuf {
        self._certificate.0.join("cert.pem")
    }
    async fn with_subject(subject: &str) -> Result<Self> {
        let directory = Directory::new()?;
        let cert = directory.0.join("cert.pem");
        let key = directory.0.join("key.pem");
        // Deliberately wrong hostname as well as an untrusted root.
        ensure!(
            Command::new("openssl")
                .args([
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-days",
                    "1",
                    "-subj",
                    "/CN=fixture.invalid",
                    "-addext",
                    &format!("subjectAltName={subject}"),
                    "-addext",
                    "basicConstraints=critical,CA:FALSE",
                    "-out"
                ])
                .arg(&cert)
                .arg("-keyout")
                .arg(&key)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success(),
            "failed to generate test certificate"
        );
        let certificates =
            CertificateDer::pem_slice_iter(&fs::read(cert)?).collect::<std::result::Result<Vec<_>, _>>()?;
        let private_key = PrivateKeyDer::from_pem_slice(&fs::read(key)?)?;
        let config =
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()?
                .with_no_client_auth()
                .with_single_cert(certificates, private_key)?;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!(
            "https://{}/subscription?token=private-test-token",
            listener.local_addr()?
        );
        let state = Arc::new(State {
            connections: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            handshake_delay_ms: AtomicU64::new(0),
            hold: AtomicBool::new(false),
            release: Semaphore::new(0),
            response: Mutex::new(format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nSubscription-Userinfo: upload=1; download=2; total=3\r\n\r\n{YAML}",
                YAML.len()
            )),
        });
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut children = JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let Ok((socket, _)) = connection else { break; };
                        shared.connections.fetch_add(1, Ordering::SeqCst);
                        let acceptor = acceptor.clone(); let state = Arc::clone(&shared);
                        children.spawn(async move {
                            sleep(Duration::from_millis(state.handshake_delay_ms.load(Ordering::SeqCst))).await;
                            let Ok(mut stream) = acceptor.accept(socket).await else { return; };
                            let mut request = Vec::new();
                            while request.len() <= 16 * 1024 && !request.ends_with(b"\r\n\r\n") {
                                match stream.read_u8().await { Ok(byte) => request.push(byte), Err(_) => return }
                            }
                            state.requests.lock().unwrap().push(String::from_utf8_lossy(&request).into_owned());
                            if state.hold.load(Ordering::SeqCst) && let Ok(permit) = state.release.acquire().await { permit.forget(); }
                            let response = state.response.lock().unwrap().clone();
                            let _ = stream.write_all(response.as_bytes()).await;
                            let _ = stream.shutdown().await;
                        });
                    }
                    _ = children.join_next(), if !children.is_empty() => {}
                }
            }
        });
        Ok(Self {
            url,
            state,
            task,
            _certificate: directory,
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
