use crate::proto::{Isync, Ping, Request, Response, Response_oneof_cmd as RespCmd, Rsync};
use protobuf::Message;
use socket2::{Domain, Protocol, Socket, Type};
use std::mem::MaybeUninit;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};
use std::{
    io::{
        Error,
        ErrorKind::{NotConnected, Other},
        Result,
    },
    thread,
};

pub struct Client {
    socket: Socket,
    server_addr: SocketAddr,
    id: String,
}

impl Client {
    pub fn new(server_addr: &str, id: &str) -> Result<Self> {
        let server_addr = server_addr
            .to_socket_addrs()?
            .next()
            .ok_or(Error::new(Other, "no addr"))?;

        let (domain, local_addr): (_, SocketAddr) = match server_addr {
            SocketAddr::V4(_) => (Domain::IPV4, "0.0.0.0:0".parse().unwrap()),
            SocketAddr::V6(_) => (Domain::IPV6, "[::]:0".parse().unwrap()),
        };

        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        socket.set_reuse_address(true)?;
        socket.bind(&local_addr.into())?;
        socket.connect(&server_addr.into())?;

        Ok(Self {
            socket: socket,
            server_addr: server_addr.into(),
            id: id.into(),
        })
    }

    fn new_req(&self) -> Request {
        let mut req = Request::new();
        req.set_id(self.id.clone());

        req
    }

    fn recv_resp(&mut self) -> Result<(Response, SocketAddr)> {
        let mut buf = unsafe { MaybeUninit::<[MaybeUninit<u8>; 1500]>::uninit().assume_init() };
        let (n, addr) = self.socket.recv_from(&mut buf)?;
        let buf = unsafe { (&buf as *const _ as *const [u8; 1500]).read() };
        let resp = Response::parse_from_bytes(&buf[..n])?;
        Ok((resp, addr.as_socket().unwrap()))
    }

    pub fn connect(&mut self, target_id: &str) -> Result<(UdpSocket, SocketAddr)> {
        let mut isync = Isync::new();
        isync.set_id(target_id.into());

        let mut req = self.new_req();
        req.set_Isync(isync);

        let isync = req.write_to_bytes()?;

        self.socket
            .set_read_timeout(Some(Duration::from_secs(10)))?;

        let mut peer_addr = None;
        for _ in 0..3 {
            let _ = self.socket.send(isync.as_ref());

            if let Ok((resp, addr)) = self.recv_resp() {
                if resp.get_id() != self.id || addr != self.server_addr {
                    continue;
                }
                match resp.cmd {
                    Some(RespCmd::Redirect(rdr)) => {
                        if rdr.addr != "" {
                            peer_addr = Some(rdr.addr);
                            break;
                        } else {
                            return Err(Error::new(Other, "target not found"));
                        }
                    }
                    _ => {}
                }
            }
        }

        if peer_addr.is_none() {
            return Err(Error::from(NotConnected));
        }

        let peer_addr: SocketAddr = peer_addr
            .unwrap()
            .parse()
            .map_err(|_| Error::from(NotConnected))?;

        println!("{} addrs {}", target_id, peer_addr.to_string());

        Ok((self.socket.try_clone().unwrap().into(), peer_addr))
    }

    pub fn listen(mut self) -> Result<UdpSocket> {
        let mut req = self.new_req();
        req.set_Ping(Ping::new());
        let ping = req.write_to_bytes()?;

        let mut last_ping: Option<Instant> = None;
        let keepalive_to = Duration::from_secs(10);
        self.socket.set_read_timeout(Some(keepalive_to))?;

        let local_addr = self.socket.local_addr().unwrap().as_socket().unwrap();

        let domain = match local_addr {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        };

        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        socket.set_reuse_address(true)?;
        socket.bind(&local_addr.into())?;

        thread::spawn(move || loop {
            if last_ping.is_none() || last_ping.as_ref().unwrap().elapsed() > keepalive_to {
                let _ = self.socket.send(ping.as_ref());
                last_ping = Some(Instant::now())
            }

            let mut buf = unsafe { MaybeUninit::<[MaybeUninit<u8>; 1500]>::uninit().assume_init() };
            let n = match self.socket.recv(&mut buf) {
                Ok(n) => n,
                Err(_) => continue,
            };

            let buf = unsafe { (&buf as *const _ as *const [u8; 1500]).read() };

            let resp = match Response::parse_from_bytes(&buf[..n]) {
                Ok(resp) => resp,
                Err(_) => continue,
            };

            if resp.get_id() != self.id {
                continue;
            }

            match resp.cmd {
                Some(RespCmd::Pong(_)) => {}
                Some(RespCmd::Fsync(fsync)) => {
                    println!("{} call me", fsync.get_id());

                    match fsync.get_addr().parse() {
                        Ok(addr) => self.send_rsync(fsync.get_id(), addr),
                        _ => continue,
                    };
                }
                _ => {}
            };
        });

        Ok(socket.into())
    }

    fn send_rsync(&mut self, id: &str, addr: SocketAddr) {
        let mut rsync = Rsync::new();
        rsync.set_id(self.id.to_string());

        let mut resp = Response::new();
        resp.set_id(id.to_string());
        resp.set_Rsync(rsync);

        let b = resp.write_to_bytes().unwrap();

        let _ = self.socket.send_to(b.as_ref(), &addr.into());
    }
}
