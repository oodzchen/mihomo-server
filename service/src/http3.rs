//! Single HTTP/3 requests carried over a SOCKS5 proxy's UDP relay.
//!
//! Browsers switch to HTTP/3 once a response advertises it (`Alt-Svc: h3`),
//! and some services answer QUIC clients differently from TCP ones, so a check
//! that should match what a browser sees has to ask over HTTP/3 too. QUIC runs
//! over UDP, which an HTTP proxy cannot carry: the datagrams go through the
//! SOCKS5 `UDP ASSOCIATE` relay of the same listener instead.
use anyhow::{Context as _, Result, bail, ensure};
use bytes::{BufMut as _, Bytes};
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpStream, UdpSocket};

/// A SOCKS5 listener that relays UDP.
#[derive(Clone)]
pub struct Relay {
    pub address: SocketAddr,
    /// Username and password, when the listener requires them.
    pub credentials: Option<(String, String)>,
}

impl fmt::Debug for Relay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Relay")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

pub struct Response {
    pub status: http::StatusCode,
    pub body: Vec<u8>,
}

/// Bodies are read up to this size.
const BODY_LIMIT: usize = 4 << 20;

/// Send `request` (an absolute `https://` URI) over HTTP/3 through `relay`.
/// Callers bound the time: an address that drops UDP never answers.
pub async fn send(relay: &Relay, request: http::Request<Bytes>) -> Result<Response> {
    let uri = request.uri();
    ensure!(uri.scheme_str() == Some("https"), "HTTP/3 needs an https URL");
    let host = uri.host().context("URL without a host")?.to_owned();
    let port = uri.port_u16().unwrap_or(443);

    // The association lasts as long as its control connection.
    let (_control, relay_address) = associate(relay).await?;
    let local = match relay_address.ip() {
        IpAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        IpAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
    };
    let socket = UdpSocket::bind(local).await.context("open a UDP socket")?;
    socket.connect(relay_address).await.context("reach the UDP relay")?;
    let socket = Arc::new(RelayedSocket {
        socket,
        header: datagram_header(&host, port)?,
        peer: relay_address,
    });

    let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        None,
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .context("open a QUIC endpoint")?;
    endpoint.set_default_client_config(client_config()?);
    let connection = endpoint
        .connect(relay_address, &host)
        .context("start the QUIC handshake")?
        .await
        .context("QUIC handshake failed")?;

    let (mut driver, mut sender) = h3::client::new(h3_quinn::Connection::new(connection.clone()))
        .await
        .context("HTTP/3 setup failed")?;
    let exchange = async {
        let (parts, body) = request.into_parts();
        let mut stream = sender
            .send_request(http::Request::from_parts(parts, ()))
            .await
            .context("HTTP/3 request failed")?;
        if !body.is_empty() {
            stream.send_data(body).await.context("HTTP/3 request failed")?;
        }
        stream.finish().await.context("HTTP/3 request failed")?;
        let response = stream.recv_response().await.context("HTTP/3 response failed")?;
        let mut body = Vec::new();
        while let Some(chunk) = stream.recv_data().await.context("HTTP/3 response failed")? {
            body.put(chunk);
            if body.len() >= BODY_LIMIT {
                break;
            }
        }
        anyhow::Ok(Response {
            status: response.status(),
            body,
        })
    };
    let result = tokio::select! {
        result = exchange => result,
        error = std::future::poll_fn(|cx| driver.poll_close(cx)) => Err(anyhow::anyhow!("HTTP/3 connection closed: {error}")),
    };
    connection.close(0u32.into(), b"");
    result
}

fn client_config() -> Result<quinn::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let mut config = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?));
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(Duration::from_secs(10).try_into()?));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// Open a UDP association; returns its control connection and the relay's address.
async fn associate(relay: &Relay) -> Result<(TcpStream, SocketAddr)> {
    let mut control = TcpStream::connect(relay.address)
        .await
        .context("connect to the SOCKS5 listener")?;
    let method = if relay.credentials.is_some() { 2 } else { 0 };
    control.write_all(&[5, 1, method]).await?;
    let mut reply = [0u8; 2];
    control.read_exact(&mut reply).await?;
    ensure!(reply == [5, method], "the SOCKS5 listener refused the login method");
    if let Some((user, password)) = &relay.credentials {
        let mut login = vec![1, u8::try_from(user.len())?];
        login.extend_from_slice(user.as_bytes());
        login.push(u8::try_from(password.len())?);
        login.extend_from_slice(password.as_bytes());
        control.write_all(&login).await?;
        control.read_exact(&mut reply).await?;
        ensure!(reply[1] == 0, "the SOCKS5 listener rejected the login");
    }
    control.write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    let mut head = [0u8; 4];
    control.read_exact(&mut head).await?;
    ensure!(head[0] == 5, "not a SOCKS5 listener");
    if head[1] != 0 {
        bail!("the SOCKS5 listener refused UDP (reply {})", head[1]);
    }
    let ip = match head[3] {
        1 => {
            let mut octets = [0u8; 4];
            control.read_exact(&mut octets).await?;
            IpAddr::from(octets)
        }
        4 => {
            let mut octets = [0u8; 16];
            control.read_exact(&mut octets).await?;
            IpAddr::from(octets)
        }
        other => bail!("unsupported SOCKS5 relay address type {other}"),
    };
    let port = control.read_u16().await?;
    // A relay bound to every address is reached where the listener is.
    let ip = if ip.is_unspecified() { relay.address.ip() } else { ip };
    Ok((control, SocketAddr::new(ip, port)))
}

/// The SOCKS5 UDP request header that sends a datagram to `host:port`.
fn datagram_header(host: &str, port: u16) -> Result<Vec<u8>> {
    let mut header = vec![0, 0, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            header.push(1);
            header.extend_from_slice(&ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            header.push(4);
            header.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            header.push(3);
            header.push(u8::try_from(host.len()).context("host name too long")?);
            header.extend_from_slice(host.as_bytes());
        }
    }
    header.extend_from_slice(&port.to_be_bytes());
    Ok(header)
}

/// The length of a relayed datagram's SOCKS5 header, `None` for a malformed or
/// fragmented one.
fn reply_header_len(datagram: &[u8]) -> Option<usize> {
    if datagram.len() < 4 || datagram[2] != 0 {
        return None;
    }
    let address = match datagram[3] {
        1 => 4,
        4 => 16,
        3 => 1 + usize::from(*datagram.get(4)?),
        _ => return None,
    };
    let length = 4 + address + 2;
    (datagram.len() >= length).then_some(length)
}

/// A UDP socket whose datagrams all go to one destination through the relay.
/// Quinn sees the relay as the peer.
struct RelayedSocket {
    socket: UdpSocket,
    header: Vec<u8>,
    peer: SocketAddr,
}

impl fmt::Debug for RelayedSocket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayedSocket")
            .field("peer", &self.peer)
            .finish()
    }
}

#[derive(Debug)]
struct Poller(Arc<RelayedSocket>);

impl quinn::UdpPoller for Poller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.0.socket.poll_send_ready(cx)
    }
}

impl quinn::AsyncUdpSocket for RelayedSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn quinn::UdpPoller>> {
        Box::pin(Poller(self))
    }

    fn try_send(&self, transmit: &quinn::udp::Transmit) -> io::Result<()> {
        let mut datagram = Vec::with_capacity(self.header.len() + transmit.contents.len());
        datagram.extend_from_slice(&self.header);
        datagram.extend_from_slice(transmit.contents);
        self.socket.try_send(&datagram).map(drop)
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [quinn::udp::RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let (Some(buf), Some(meta)) = (bufs.first_mut(), meta.first_mut()) else {
            return Poll::Ready(Ok(0));
        };
        loop {
            let mut read = tokio::io::ReadBuf::new(&mut buf[..]);
            std::task::ready!(self.socket.poll_recv(cx, &mut read))?;
            let received = read.filled().len();
            let Some(header) = reply_header_len(read.filled()) else {
                continue;
            };
            buf.copy_within(header..received, 0);
            let length = received - header;
            *meta = quinn::udp::RecvMeta::default();
            meta.addr = self.peer;
            meta.len = length;
            meta.stride = length;
            return Poll::Ready(Ok(1));
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datagram_headers_name_the_destination() {
        assert_eq!(
            datagram_header("a.io", 443).unwrap(),
            [0, 0, 0, 3, 4, b'a', b'.', b'i', b'o', 1, 187]
        );
        assert_eq!(datagram_header("1.2.3.4", 80).unwrap(), [0, 0, 0, 1, 1, 2, 3, 4, 0, 80]);
    }

    #[test]
    fn reply_headers_are_measured_and_fragments_dropped() {
        assert_eq!(reply_header_len(&[0, 0, 0, 1, 1, 2, 3, 4, 0, 80, 9]), Some(10));
        assert_eq!(reply_header_len(&[0, 0, 0, 3, 1, b'a', 0, 80]), Some(8));
        assert_eq!(reply_header_len(&[0, 0, 1, 1, 1, 2, 3, 4, 0, 80]), None);
        assert_eq!(reply_header_len(&[0, 0, 0, 1, 1, 2]), None);
    }
}
