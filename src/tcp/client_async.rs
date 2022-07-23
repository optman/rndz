use crate::proto::{
    Isync, Ping, Request, Request_oneof_cmd as ReqCmd, Response, Response_oneof_cmd as RespCmd,
    Rsync,
};
use protobuf::Message;
use std::io::{Error, ErrorKind::Other, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    join,
    net::{
        lookup_host,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpSocket, TcpStream,
    },
    select,
    sync::mpsc::{channel, Receiver, Sender},
    sync::Notify,
    task::spawn,
    time::{sleep, timeout},
};

/// Async tcp connection builder
///
/// async version of [`crate::tcp::Client`],
/// require tokio async runtime
///
pub struct Client {
    server_addr: String,
    id: String,
    listener: Option<TcpListener>,
    local_addr: Option<SocketAddr>,
    exit: Arc<Notify>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.exit.notify_waiters();
    }
}

impl Client {
    /// set rendezvous server, peer identity, local bind address.
    /// if no local address set, choose according server address type(ipv4 or ipv6).
    pub fn new(server_addr: &str, id: &str, local_addr: Option<SocketAddr>) -> Result<Self> {
        Ok(Self {
            server_addr: server_addr.to_owned(),
            id: id.into(),
            local_addr: local_addr,
            listener: None,
            exit: Default::default(),
        })
    }

    /// expose TcpListener
    pub fn as_socket(&self) -> Option<&TcpListener> {
        self.listener.as_ref()
    }

    async fn choose_bind_addr(&self) -> Result<SocketAddr> {
        if let Some(ref addr) = self.local_addr {
            return Ok(*addr);
        }

        let server_addr = lookup_host(self.server_addr.clone())
            .await?
            .next()
            .ok_or(Error::new(Other, "server name resolve fail"))?;

        let local_addr = match server_addr {
            SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
            SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
        };

        Ok(local_addr)
    }

    async fn connect_server(local_addr: SocketAddr, server_addr: &str) -> Result<TcpStream> {
        let svr = Self::bind(local_addr.into())?;
        let server_addr = lookup_host(server_addr)
            .await?
            .next()
            .ok_or(Error::new(Other, "server name resolve fail"))?;
        svr.connect(server_addr).await
    }

    fn bind(local_addr: SocketAddr) -> Result<TcpSocket> {
        let s = match local_addr {
            SocketAddr::V4(_) => TcpSocket::new_v4(),
            SocketAddr::V6(_) => TcpSocket::new_v6(),
        }?;

        s.set_reuseaddr(true)?;
        #[cfg(unix)]
        s.set_reuseport(true)?;
        s.bind(local_addr)?;

        Ok(s)
    }

    /// connect to rendezvous server and request a connection to target node.
    ///
    /// it will return a TcpStream with remote peer.
    ///
    /// the connection with rendezvous server will be drop after return.
    ///
    pub async fn connect(&mut self, target_id: &str) -> Result<TcpStream> {
        let svr = Self::connect_server(self.choose_bind_addr().await?, &self.server_addr).await?;
        let local_addr = svr.local_addr().unwrap();
        let (mut r, mut w) = svr.into_split();

        let mut isync = Isync::new();
        isync.set_id(target_id.into());

        Self::write_req(self.id.clone(), &mut w, ReqCmd::Isync(isync)).await?;

        let addr = match Self::read_resp(&mut r).await?.cmd {
            Some(RespCmd::Redirect(rdr)) => rdr.addr,
            _ => Err(Error::new(Other, "invalid server response"))?,
        };

        log::debug!("Redirect {}", addr);

        let target_addr: SocketAddr = addr
            .parse()
            .map_err(|_| Error::new(Other, "target id not found"))?;

        let s = Self::bind(local_addr)?;
        s.connect(target_addr).await
    }

    fn new_req(id: String) -> Request {
        let mut req = Request::new();
        req.set_id(id);

        req
    }

    async fn write_req(id: String, w: &mut OwnedWriteHalf, cmd: ReqCmd) -> Result<()> {
        let mut req = Self::new_req(id);
        req.cmd = Some(cmd);
        let buf = req.write_to_bytes()?;

        w.write_all(&(buf.len() as u16).to_be_bytes()).await?;
        w.write_all(&buf).await?;
        Ok(())
    }

    async fn read_resp(r: &mut OwnedReadHalf) -> Result<Response> {
        let mut buf = [0; 2];
        r.read_exact(&mut buf).await?;
        let mut buf = vec![0; u16::from_be_bytes(buf).into()];
        r.read_exact(&mut buf).await?;
        Response::parse_from_bytes(&buf).map_err(|_| Error::new(Other, "invalid message"))
    }

    async fn handle_resp(local_addr: SocketAddr, r: &mut OwnedReadHalf) -> Result<Option<ReqCmd>> {
        let resp = Self::read_resp(r).await;
        match resp {
            Ok(resp) => match resp.cmd {
                Some(RespCmd::Pong(_)) => Ok(None),
                Some(RespCmd::Fsync(fsync)) => {
                    log::debug!("fsync {}", fsync.get_id());

                    let dst_addr: SocketAddr = fsync
                        .get_addr()
                        .parse()
                        .map_err(|_| Error::new(Other, "invalid fsync addr"))?;

                    if let Ok(s) = Self::bind(local_addr.into()) {
                        log::debug!("connect {} -> {}", local_addr, dst_addr);
                        let _ = timeout(Duration::from_micros(1), s.connect(dst_addr)).await;
                    }

                    let mut rsync = Rsync::new();
                    rsync.set_id(fsync.get_id().to_string());
                    Ok(Some(ReqCmd::Rsync(rsync)))
                }
                _ => Ok(None),
            },
            Err(e) => Err(e),
        }
    }

    async fn read_loop(
        exit: Arc<Notify>,
        local_addr: SocketAddr,
        mut r: OwnedReadHalf,
        tx: Sender<ReqCmd>,
    ) {
        loop {
            let resp = select! {
                 _ = exit.notified() =>  break,
                 req= Self::handle_resp(local_addr, &mut r) => req,
            };

            let ok = match resp {
                Ok(None) => true,
                Ok(Some(req)) => tx.send(req).await.is_ok(),
                Err(_) => false,
            };

            if !ok {
                break;
            }
        }
    }

    async fn write_loop(
        exit: Arc<Notify>,
        id: String,
        mut w: OwnedWriteHalf,
        mut rx: Receiver<ReqCmd>,
    ) {
        loop {
            let req = select! {
                 _ = exit.notified() =>  break,
                 _ = sleep(Duration::from_secs(10)) => Some(ReqCmd::Ping(Ping::new())),
                 req = rx.recv() => req,
            };

            if req.is_none()
                || Self::write_req(id.clone(), &mut w, req.unwrap())
                    .await
                    .is_err()
            {
                break;
            }
        }
    }

    async fn start_background(
        id: String,
        local_addr: SocketAddr,
        server_addr: String,
        exit: Arc<Notify>,
    ) -> Result<Arc<Notify>> {
        let svr_sk = Self::connect_server(local_addr, &server_addr).await?;
        let (r, w) = svr_sk.into_split();
        let (tx, rx) = channel(10);

        let broken: Arc<Notify> = Default::default();

        tx.send(ReqCmd::Ping(Ping::new())).await.unwrap();

        let rl = {
            let exit = exit.clone();
            spawn(async move {
                Self::read_loop(exit, local_addr, r, tx).await;
            })
        };

        let wl = {
            let exit = exit.clone();
            let id = id.clone();
            spawn(async move {
                Self::write_loop(exit, id, w, rx).await;
            })
        };

        {
            let broken = broken.clone();
            spawn(async move {
                let _ = join!(rl, wl);

                broken.notify_waiters();
            });
        }

        Ok(broken)
    }

    /// put socket in listen mode, create connection with rendezvous server, wait for peer
    /// connection request. if connection with server broken it will auto reconnect.
    ///
    /// when received `Fsync` request from server, attempt to connect remote peer with a very short timeout,
    /// this will open the firwall and nat rule for the peer connection that will follow immediately.
    /// When the peer connection finally come, the listening socket then accept it as normal.
    pub async fn listen(&mut self) -> Result<()> {
        let listener = Self::bind(self.choose_bind_addr().await?)?;
        let local_addr = listener.local_addr().unwrap();
        let listener = listener.listen(10)?;

        let id = self.id.clone();
        let server_addr = self.server_addr.clone();
        let exit = self.exit.clone();
        let mut broken =
            Self::start_background(id.clone(), local_addr, server_addr.clone(), exit.clone())
                .await?;

        spawn(async move {
            loop {
                select! {
                    _ = exit.notified() => break,
                    _ = broken.notified() => {},
                };

                log::debug!("connection with server is broken, try to reconnect");

                broken = loop {
                    match Self::start_background(
                        id.clone(),
                        local_addr,
                        server_addr.clone(),
                        exit.clone(),
                    )
                    .await
                    {
                        Ok(broken) => {
                            log::debug!("connect server success");
                            break broken;
                        }
                        Err(err) => {
                            log::debug!("connect server fail, retry later. {}", err)
                        }
                    };

                    select! {
                        _ = exit.notified() => return,
                        _ = sleep(Duration::from_secs(120)) =>{},
                    };
                };
            }
        });

        self.listener = Some(listener);

        Ok(())
    }

    /// accept remote peer connection
    pub async fn accept(&mut self) -> Result<(TcpStream, SocketAddr)> {
        select! {
            _ = self.exit.notified() => Err(Error::new(Other, "exit")),

            r  = self.listener
                    .as_ref()
                    .ok_or(Error::new(Other, "not listening"))?
                    .accept() => r,
        }
    }
}
