use crate::proto::{
    Isync, Ping, Request, Request_oneof_cmd as ReqCmd, Response, Response_oneof_cmd as RespCmd,
    Rsync,
};
use protobuf::Message;
use socket2::{Domain, Protocol, Socket, Type};
use std::io::{Error, ErrorKind::Other, Read, Result, Write};
use std::net::{Shutdown::Both, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError::Timeout, SyncSender};
use std::thread::spawn;
use std::time::Duration;

pub struct Client {
    server_addr: String,
    id: String,
    listener: Option<Socket>,
    svr_sk: Option<Socket>,
    local_addr: Option<SocketAddr>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown().unwrap();
    }
}

impl Client {
    pub fn new(server_addr: &str, id: &str, local_addr: Option<SocketAddr>) -> Result<Self> {
        Ok(Self {
            server_addr: server_addr.to_owned(),
            id: id.into(),
            local_addr: local_addr,
            listener: None,
            svr_sk: None,
        })
    }

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
            .ok_or(Error::new(Other, "server name resolve fail"))?;

        let local_addr = match server_addr {
            SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
            SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
        };

        Ok(local_addr)
    }

    fn connect_server(&mut self, local_addr: Option<SocketAddr>) -> Result<socket2::Socket> {
        let svr = Self::bind(local_addr.unwrap_or(self.choose_bind_addr()?).into())?;
        let server_addr = self
            .server_addr
            .to_socket_addrs()?
            .next()
            .ok_or(Error::new(Other, "server name resolve fail"))?;
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

    pub fn connect(&mut self, target_id: &str) -> Result<TcpStream> {
        let mut svr = self.connect_server(None)?;

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
        let mut buf = [0u8; 2];
        r.read_exact(&mut buf)?;
        let mut buf = vec![0; u16::from_be_bytes(buf).into()];
        r.read_exact(&mut buf)?;
        Response::parse_from_bytes(&buf).map_err(|_| Error::new(Other, "invalid message"))
    }

    fn write_loop(
        id: String,
        s: &mut dyn Write,
        listener: Socket,
        rx: Receiver<ReqCmd>,
    ) -> Result<()> {
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

        let _ = listener.shutdown(Both);

        Ok(())
    }

    fn read_loop(
        local_addr: SocketAddr,
        r: &mut dyn Read,
        listener: Socket,
        tx: SyncSender<ReqCmd>,
    ) -> Result<()> {
        loop {
            let req = match Self::read_resp(r) {
                Ok(resp) => match resp.cmd {
                    Some(RespCmd::Pong(_)) => None,
                    Some(RespCmd::Fsync(fsync)) => {
                        log::debug!("fsync {}", fsync.get_id());

                        let dst_addr: SocketAddr = fsync
                            .get_addr()
                            .parse()
                            .map_err(|_| Error::new(Other, "invalid fsync addr"))?;

                        log::debug!("connect {} -> {}", local_addr, dst_addr);

                        let _ = Self::bind(local_addr.into())
                            .map(|s| s.connect_timeout(&dst_addr.into(), Duration::from_micros(1)));

                        let mut rsync = Rsync::new();
                        rsync.set_id(fsync.get_id().to_string());
                        Some(ReqCmd::Rsync(rsync))
                    }
                    _ => None,
                },
                _ => break,
            };

            if let Some(req) = req {
                tx.send(req).unwrap();
            }
        }

        let _ = listener.shutdown(Both);

        Ok(())
    }

    pub fn listen(&mut self) -> Result<()> {
        let listener = Self::bind(self.choose_bind_addr()?)?;
        listener.listen(10)?;

        let local_addr = listener.local_addr().unwrap().as_socket().unwrap();

        let svr_sk = self.connect_server(Some(local_addr))?;

        let (tx, rx) = sync_channel(10);

        tx.send(ReqCmd::Ping(Ping::new())).unwrap();

        {
            let id = self.id.clone();
            let mut w = svr_sk.try_clone()?;
            let l = listener.try_clone().unwrap();
            spawn(move || Self::write_loop(id, &mut w, l, rx).unwrap());
        }

        {
            let local_addr = local_addr.clone();
            let mut r = svr_sk.try_clone()?;
            let l = listener.try_clone().unwrap();
            spawn(move || Self::read_loop(local_addr, &mut r, l, tx).unwrap());
        }

        self.listener = Some(listener);
        self.svr_sk = Some(svr_sk);

        Ok(())
    }

    pub fn accept(&mut self) -> Result<(TcpStream, SocketAddr)> {
        self.listener
            .as_ref()
            .ok_or(Error::new(Other, "not listening"))?
            .accept()
            .map(|(s, a)| (s.into(), a.as_socket().unwrap()))
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let _ = self.listener.take().map(|l| l.shutdown(Both));
        let _ = self.svr_sk.take().map(|s| s.shutdown(Both));
        Ok(())
    }
}
