use crate::proto::{
    Isync, Ping, Request, Request_oneof_cmd as ReqCmd, Response, Response_oneof_cmd as RespCmd,
};
use protobuf::Message;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::io::{Error, ErrorKind::Other, Read, Result, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::thread;
use std::time::Duration;

pub struct Client {
    server_addr: String,
    id: String,
    listener: Option<Socket>,
}

impl Client {
    pub fn new(server_addr: &str, id: &str) -> Result<Self> {
        Ok(Self {
            server_addr: server_addr.into(),
            id: id.into(),
            listener: None,
        })
    }

    fn connect_server(&mut self) -> Result<socket2::Socket> {
        let server_addrs = self.server_addr.to_socket_addrs()?;

        let mut last_error = None;

        for a in server_addrs {
            let (domain, local_addr): (_, SocketAddr) = match a {
                SocketAddr::V4(_) => (Domain::IPV4, "0.0.0.0:0".parse().unwrap()),
                SocketAddr::V6(_) => (Domain::IPV6, "[::]:".parse().unwrap()),
            };

            let svr = Socket::new(domain, Type::STREAM, Some(Protocol::TCP)).unwrap();
            svr.set_reuse_address(true)?;
            svr.set_reuse_port(true)?;

            svr.bind(&local_addr.into())?;

            match svr.connect(&a.into()) {
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

        println!("{} at {}", target_id, addr);

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
        let mut svr = self.connect_server()?;
        let local_addr = svr.local_addr().unwrap().as_socket().unwrap();

        let ping_fn = |id: String, s: &mut dyn Write| -> Result<()> {
            loop {
                Self::write_req(id.clone(), s, ReqCmd::Ping(Ping::new()))?;
                thread::sleep(Duration::from_secs(10));
            }
        };

        let id = self.id.clone();
        let mut w = svr.try_clone()?;
        thread::spawn(move || {
            ping_fn(id, &mut w).unwrap();
        });

        let recv_fn = |local_addr: SocketAddr, r: &mut dyn Read| -> Result<()> {
            loop {
                match Self::read_resp(r)?.cmd {
                    Some(RespCmd::Pong(_)) => println!("pong"),
                    Some(RespCmd::Fsync(fsync)) => {
                        let dst_addr: SocketAddr = fsync
                            .get_addr()
                            .parse()
                            .map_err(|_| Error::new(Other, "invalid fsync addr"))?;
                        println!("{} call me", dst_addr.to_string());

                        let s = Self::bind(local_addr.clone().into())?;
                        let _ = s.connect_timeout(&dst_addr.into(), Duration::from_secs(1));
                    }
                    _ => continue,
                }
            }
        };

        {
            let local_addr = local_addr.clone();
            thread::spawn(move || {
                recv_fn(local_addr, &mut svr).unwrap();
            });
        }

        println!("i am listening at {}", local_addr.to_string());
        let s = Self::bind(local_addr.into())?;
        s.listen(1)?;

        self.listener = Some(s);

        Ok(())
    }

    pub fn accept(&mut self) -> Result<(TcpStream, SocketAddr)> {
        self.listener
            .as_ref()
            .ok_or(Error::new(Other, "not listening"))?
            .accept()
            .map(|(s, a)| (s.into(), a.as_socket().unwrap()))
    }
}
