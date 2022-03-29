use crate::proto::{
    Isync, Ping, Request, Request_oneof_cmd as ReqCmd, Response, Response_oneof_cmd as RespCmd,
};
use protobuf::Message;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io::{Error, ErrorKind::Other, Read, Result, Write};
use std::net::{Shutdown::Both, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::spawn;
use std::time::Duration;

pub struct Client {
    server_addr: String,
    id: String,
    listener: Option<Socket>,
    svr_sk: Option<Socket>,
    local_addr: Option<SocketAddr>,
    exit: Arc<(Mutex<bool>, Condvar)>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown().unwrap();
    }
}

impl Client {
    pub fn new(server_addr: &str, id: &str, local_addr: Option<SocketAddr>) -> Result<Self> {
        Ok(Self {
            server_addr: server_addr.into(),
            id: id.into(),
            local_addr: local_addr,
            listener: None,
            svr_sk: None,
            exit: Default::default(),
        })
    }

    fn connect_server(&mut self) -> Result<socket2::Socket> {
        let server_addrs = self.server_addr.to_socket_addrs()?;

        let mut last_error = None;

        for server_addr in server_addrs {
            let local_addr = match self.local_addr {
                Some(addr) => addr,
                None => match server_addr {
                    SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
                    SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
                },
            };

            let domain = match local_addr {
                SocketAddr::V4(_) => Domain::IPV4,
                SocketAddr::V6(_) => Domain::IPV6,
            };

            let svr = Socket::new(domain, Type::STREAM, Some(Protocol::TCP)).unwrap();
            svr.set_reuse_address(true)?;
            #[cfg(unix)]
            svr.set_reuse_port(true)?;

            svr.bind(&local_addr.into())?;

            match svr.connect(&server_addr.into()) {
                Ok(..) => return Ok(svr),
                Err(e) => last_error = Some(e),
            }
        }

        Err(last_error.unwrap_or(Error::new(Other, "server name resolve fail")))
    }

    fn bind(local_addr: SockAddr) -> Result<Socket> {
        let domain = match local_addr.as_socket().unwrap() {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        };

        let s = Socket::new(domain, Type::STREAM, Some(Protocol::TCP)).unwrap();
        s.set_reuse_address(true)?;
        #[cfg(unix)]
        s.set_reuse_port(true)?;
        s.bind(&local_addr)?;

        Ok(s)
    }

    pub fn connect(&mut self, target_id: &str) -> Result<TcpStream> {
        let mut svr = self.connect_server()?;

        let mut isync = Isync::new();
        isync.set_id(target_id.into());

        Self::write_req(self.id.clone(), &mut svr, ReqCmd::Isync(isync))?;

        let addr = match Self::read_resp(&mut svr)?.cmd {
            Some(RespCmd::Redirect(rdr)) => rdr.addr,
            _ => Err(Error::new(Other, "invalid server response"))?,
        };

        let target_addr: SocketAddr = addr
            .parse()
            .map_err(|_| Error::new(Other, "target id not found"))?;

        let local_addr = svr.local_addr().unwrap();

        let s = Self::bind(local_addr)?;
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

    pub fn listen(&mut self) -> Result<()> {
        let svr_sk = self.connect_server()?;
        let local_addr = svr_sk.local_addr().unwrap().as_socket().unwrap();

        let listener = Self::bind(local_addr.into())?;
        listener.listen(1)?;

        let ping_fn = |exit: Arc<(Mutex<bool>, Condvar)>,
                       id: String,
                       s: &mut dyn Write,
                       listener: Socket|
         -> Result<()> {
            loop {
                let (exit, cond) = &*exit;
                if *exit.lock().unwrap() {
                    break;
                }

                match Self::write_req(id.clone(), s, ReqCmd::Ping(Ping::new())) {
                    Ok(_) => {
                        let _ = cond.wait_timeout(exit.lock().unwrap(), Duration::from_secs(10));
                    }
                    Err(_) => {
                        listener.shutdown(Both).unwrap();
                        break;
                    }
                }
            }

            Ok(())
        };

        let id = self.id.clone();
        let mut w = svr_sk.try_clone()?;
        let exit = self.exit.clone();
        let l = listener.try_clone().unwrap();
        spawn(move || {
            ping_fn(exit, id, &mut w, l).unwrap();
        });

        let recv_fn = |exit: Arc<(Mutex<bool>, Condvar)>,
                       local_addr: SocketAddr,
                       r: &mut dyn Read,
                       listener: Socket|
         -> Result<()> {
            loop {
                let (exit, _) = &*exit;
                if *exit.lock().unwrap() {
                    break;
                }
                match Self::read_resp(r) {
                    Ok(resp) => match resp.cmd {
                        Some(RespCmd::Pong(_)) => {}
                        Some(RespCmd::Fsync(fsync)) => {
                            let dst_addr: SocketAddr = fsync
                                .get_addr()
                                .parse()
                                .map_err(|_| Error::new(Other, "invalid fsync addr"))?;

                            let _ = Self::bind(local_addr.clone().into())
                                .map(|s| {
                                    s.connect_timeout(&dst_addr.into(), Duration::from_secs(1))
                                })
                                .map_err(|e| println!("{}", e));
                        }
                        _ => continue,
                    },
                    Err(e) => {
                        println!("rndz server read fail. {}", e);
                        let _ = listener.shutdown(Both);
                        break;
                    }
                }
            }
            Ok(())
        };

        {
            let exit = self.exit.clone();
            let local_addr = local_addr.clone();
            let mut r = svr_sk.try_clone()?;
            let l = listener.try_clone().unwrap();
            spawn(move || {
                let _ = recv_fn(exit, local_addr, &mut r, l);
            });
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
        let (exit, cond) = &*self.exit;
        *exit.lock().unwrap() = true;
        cond.notify_all();

        let _ = self.listener.take().map(|l| l.shutdown(Both));
        let _ = self.svr_sk.take().map(|s| s.shutdown(Both));
        Ok(())
    }
}
