use crate::proto::{
    Isync, Ping, Request, Request_oneof_cmd as ReqCmd, Response, Response_oneof_cmd as RespCmd,
    Rsync,
};
use protobuf::Message;
use socket2::{Domain, Protocol, Socket, Type};
use std::io::{Error, ErrorKind::Other, Read, Result, Write};
use std::net::{Shutdown::Both, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError::Timeout, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::spawn;
use std::time::Duration;

#[derive(Clone, Default)]
struct Signal {
    exit: bool,
    broken: bool,
}

/// Tcp connection builder
///
/// Bind to a port, connect to a rendezvous server. Wait for peer connection request or initiate a
/// peer connection request.
///
/// # example
///
/// ```no_run
/// use rndz::tcp::Client;
///
/// let mut c1 = Client::new("rndz_server:1234", "c1", None).unwrap();
/// c1.listen().unwrap();
/// while let Ok((stream, addr)) = c1.accept(){
/// //...
/// }
/// ```
///
/// client2
/// ```no_run
/// use rndz::tcp::Client;
/// let mut c2 = Client::new("rndz_server:1234", "c2", None).unwrap();
/// let stream = c2.connect("c1").unwrap();
/// ```
///
pub struct Client {
    server_addr: String,
    id: String,
    listener: Option<Socket>,
    local_addr: Option<SocketAddr>,
    signal: Arc<(Mutex<Signal>, Condvar)>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown().unwrap();
    }
}

impl Client {
    /// set rendezvous server, peer identity, local bind address.
    /// if no local address set, choose according server address type(ipv4 or ipv6).
    pub fn new(server_addr: &str, id: &str, local_addr: Option<SocketAddr>) -> Result<Self> {
        Ok(Self {
            server_addr: server_addr.to_owned(),
            id: id.into(),
            local_addr,
            listener: None,
            signal: Default::default(),
        })
    }

    /// expose TcpListener
    pub fn as_socket(&self) -> Option<TcpListener> {
        self.listener
            .as_ref()
            .map(|l| l.try_clone().unwrap().into())
    }

    fn choose_bind_addr(&self) -> Result<SocketAddr> {
        if let Some(ref addr) = self.local_addr {
            return Ok(*addr);
        }

        let server_addr = self
            .server_addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| Error::new(Other, "server name resolve fail"))?;

        let local_addr = match server_addr {
            SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
            SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
        };

        Ok(local_addr)
    }

    fn connect_server(local_addr: SocketAddr, server_addr: &str) -> Result<socket2::Socket> {
        let svr = Self::bind(local_addr)?;
        let server_addr = server_addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| Error::new(Other, "server name resolve fail"))?;
        svr.connect(&server_addr.into())?;
        Ok(svr)
    }

    fn bind(local_addr: SocketAddr) -> Result<Socket> {
        let domain = match local_addr {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        };

        let s = Socket::new(domain, Type::STREAM, Some(Protocol::TCP)).unwrap();
        s.set_reuse_address(true)?;
        #[cfg(unix)]
        s.set_reuse_port(true)?;
        s.bind(&local_addr.into())?;

        Ok(s)
    }

    /// connect to rendezvous server and request a connection to target node.
    ///
    /// it will return a TcpStream with remote peer.
    ///
    /// the connection with rendezvous server will be drop after return.
    ///
    pub fn connect(&mut self, target_id: &str) -> Result<TcpStream> {
        let mut svr = Self::connect_server(self.choose_bind_addr()?, &self.server_addr)?;

        let mut isync = Isync::new();
        isync.set_id(target_id.into());

        Self::write_req(self.id.clone(), &mut svr, ReqCmd::Isync(isync))?;

        let addr = match Self::read_resp(&mut svr)?.cmd {
            Some(RespCmd::Redirect(rdr)) => rdr.addr,
            _ => Err(Error::new(Other, "invalid server response"))?,
        };

        log::debug!("Redirect {}", addr);

        let target_addr: SocketAddr = addr
            .parse()
            .map_err(|_| Error::new(Other, "target id not found"))?;

        let local_addr = svr.local_addr().unwrap();

        let s = Self::bind(local_addr.as_socket().unwrap())?;
        s.connect(&target_addr.into())?;

        Ok(s.into())
    }

    fn new_req(id: String) -> Request {
        let mut req = Request::new();
        req.set_id(id);

        req
    }

    fn write_req(id: String, w: &mut dyn Write, cmd: ReqCmd) -> Result<()> {
        let mut req = Self::new_req(id);
        req.cmd = Some(cmd);
        let buf = req.write_to_bytes()?;

        w.write_all(&(buf.len() as u16).to_be_bytes())?;
        w.write_all(&buf)?;
        Ok(())
    }

    fn read_resp(r: &mut dyn Read) -> Result<Response> {
        let mut buf = [0; 2];
        r.read_exact(&mut buf)?;
        let mut buf = vec![0; u16::from_be_bytes(buf).into()];
        r.read_exact(&mut buf)?;
        Response::parse_from_bytes(&buf).map_err(|_| Error::new(Other, "invalid message"))
    }

    fn write_loop(id: String, s: &mut dyn Write, rx: Receiver<ReqCmd>) -> Result<()> {
        loop {
            let req = match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(req) => req,
                Err(Timeout) => ReqCmd::Ping(Ping::new()),
                _ => break,
            };

            if Self::write_req(id.clone(), s, req).is_err() {
                break;
            }
        }

        Ok(())
    }

    fn read_loop(local_addr: SocketAddr, r: &mut dyn Read, tx: SyncSender<ReqCmd>) -> Result<()> {
        while let Ok(resp) = Self::read_resp(r) {
            let req = match resp.cmd {
                Some(RespCmd::Pong(_)) => None,
                Some(RespCmd::Fsync(fsync)) => {
                    log::debug!("fsync {}", fsync.get_id());

                    let dst_addr: SocketAddr = fsync
                        .get_addr()
                        .parse()
                        .map_err(|_| Error::new(Other, "invalid fsync addr"))?;

                    log::debug!("connect {} -> {}", local_addr, dst_addr);

                    let _ = Self::bind(local_addr)
                        .map(|s| s.connect_timeout(&dst_addr.into(), Duration::from_micros(1)));

                    let mut rsync = Rsync::new();
                    rsync.set_id(fsync.get_id().to_string());
                    Some(ReqCmd::Rsync(rsync))
                }
                _ => None,
            };

            if let Some(req) = req {
                tx.send(req).unwrap();
            };
        }

        Ok(())
    }

    fn start_background(
        id: String,
        local_addr: SocketAddr,
        server_addr: String,
        signal: Arc<(Mutex<Signal>, Condvar)>,
    ) -> Result<()> {
        let svr_sk = Self::connect_server(local_addr, &server_addr)?;

        let (tx, rx) = sync_channel(10);

        tx.send(ReqCmd::Ping(Ping::new())).unwrap();

        let mut hs = vec![];

        hs.push({
            let mut w = svr_sk.try_clone()?;
            spawn(move || Self::write_loop(id, &mut w, rx).unwrap())
        });

        hs.push({
            let mut r = svr_sk.try_clone()?;
            spawn(move || Self::read_loop(local_addr, &mut r, tx).unwrap())
        });

        {
            let signal = signal.clone();
            spawn(move || {
                for h in hs {
                    h.join().unwrap();
                }
                let (lock, cvar) = &*signal;
                let mut signal = lock.lock().unwrap();
                signal.broken = true;
                cvar.notify_all();
            });
        }

        {
            spawn(move || {
                let (lock, cvar) = &*signal;
                let mut signal = lock.lock().unwrap();
                if signal.exit {
                    let _ = svr_sk.shutdown(Both);
                    return;
                }
                signal = cvar.wait(signal).unwrap();
                if signal.exit {
                    let _ = svr_sk.shutdown(Both);
                }
            });
        }

        Ok(())
    }

    /// put socket in listen mode, create connection with rendezvous server, wait for peer
    /// connection request. if connection with server broken it will auto reconnect.
    ///
    /// when received `Fsync` request from server, attempt to connect remote peer with a very short timeout,
    /// this will open the firwall and nat rule for the peer connection that will follow immediately.
    /// When the peer connection finally come, the listening socket then accept it as normal.
    pub fn listen(&mut self) -> Result<()> {
        let listener = Self::bind(self.choose_bind_addr()?)?;
        listener.listen(10)?;

        let id = self.id.clone();
        let local_addr = listener.local_addr().unwrap().as_socket().unwrap();
        let server_addr = self.server_addr.clone();
        let signal = self.signal.clone();
        Self::start_background(id.clone(), local_addr, server_addr.clone(), signal.clone())?;

        spawn(move || loop {
            {
                let (lock, cvar) = &*signal;
                let mut signal = lock.lock().unwrap();
                if signal.exit {
                    return;
                }
                signal = cvar.wait(signal).unwrap();
                if signal.exit {
                    return;
                }

                assert!(signal.broken);

                signal.broken = false;
            }

            log::debug!("connection with server is broken, try to reconnect.");

            loop {
                match Self::start_background(
                    id.clone(),
                    local_addr,
                    server_addr.clone(),
                    signal.clone(),
                ) {
                    Ok(_) => {
                        log::debug!("connect server success");
                        break;
                    }
                    Err(err) => log::debug!("connect server fail, retry later. {}", err),
                };

                let (lock, cvar) = &*signal;
                let mut signal = lock.lock().unwrap();
                if signal.exit {
                    return;
                }
                signal = cvar
                    .wait_timeout(signal, Duration::from_secs(120))
                    .unwrap()
                    .0;
                if signal.exit {
                    return;
                }
            }
        });

        self.listener = Some(listener);

        Ok(())
    }

    /// accept remote peer connection
    pub fn accept(&mut self) -> Result<(TcpStream, SocketAddr)> {
        self.listener
            .as_ref()
            .ok_or_else(|| Error::new(Other, "not listening"))?
            .accept()
            .map(|(s, a)| (s.into(), a.as_socket().unwrap()))
    }

    /// stop internal listen thread.
    pub fn shutdown(&mut self) -> Result<()> {
        let _ = self.listener.take().map(|l| l.shutdown(Both));

        let (lock, cvar) = &*self.signal;
        let mut signal = lock.lock().unwrap();
        signal.exit = true;
        cvar.notify_all();

        Ok(())
    }
}
