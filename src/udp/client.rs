use crate::proto::{
    Isync, Ping, Request, Request_oneof_cmd as ReqCmd, Response, Response_oneof_cmd as RespCmd,
    Rsync,
};
use protobuf::Message;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};
use std::{
    io::{
        Error,
        ErrorKind::{NotConnected, Other},
        Result,
    },
    sync::mpsc::{sync_channel, Receiver},
    thread,
};

pub struct Client {
    socket: UdpSocket,
    server_addr: SocketAddr,
    id: String,
}

impl Client {
    pub fn new(server_addr: &str, id: &str) -> Result<Self> {
        let server_addr = server_addr
            .to_socket_addrs()?
            .next()
            .ok_or(Error::new(Other, "no addr"))?;

        let bind_addr = match server_addr {
            SocketAddr::V4(_) => "0.0.0.0:0",
            SocketAddr::V6(_) => "[::]:0",
        };

        let socket = UdpSocket::bind(bind_addr)?;

        Self::with_socket(socket, server_addr, id)
    }

    pub fn with_socket(socket: UdpSocket, server_addr: SocketAddr, id: &str) -> Result<Self> {
        Ok(Self {
            socket: socket,
            server_addr: server_addr,
            id: id.into(),
        })
    }

    fn rebind(&mut self) -> Result<()> {
        let bind_addr = match self.socket.local_addr().unwrap() {
            SocketAddr::V4(_) => "0.0.0.0:0",
            SocketAddr::V6(_) => "[::]:0",
        };

        self.socket = UdpSocket::bind(bind_addr)?;

        Ok(())
    }

    fn new_req(&self) -> Request {
        let mut req = Request::new();
        req.set_id(self.id.clone());

        req
    }

    fn recv_resp(&mut self) -> Result<(Response, SocketAddr)> {
        let mut buf = [0; 1500];
        let (n, addr) = self.socket.recv_from(&mut buf)?;
        let resp = Response::parse_from_bytes(&mut buf[..n])?;
        Ok((resp, addr))
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
            let _ = self.socket.send_to(isync.as_ref(), self.server_addr);

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

        for _ in 0..3 {
            let _ = self.socket.send_to(isync.as_ref(), peer_addr);

            if let Ok((resp, addr)) = self.recv_resp() {
                if resp.id != self.id || addr != peer_addr {
                    continue;
                }
                match resp.cmd {
                    Some(RespCmd::Rsync(rsync)) => {
                        if rsync.id == target_id.as_ref() {
                            return Ok((self.socket.try_clone().unwrap(), peer_addr));
                        }
                    }
                    _ => {}
                }
            }
        }

        Err(Error::from(NotConnected))
    }

    pub fn listen(mut self) -> Result<Acceptor<(UdpSocket, SocketAddr)>> {
        let mut req = self.new_req();
        req.set_Ping(Ping::new());
        let ping = req.write_to_bytes()?;

        let mut last_ping: Option<Instant> = None;
        let keepalive_to = Duration::from_secs(10);
        self.socket.set_read_timeout(Some(keepalive_to))?;

        let mut last_peer_addr = None;

        let (tx, rx) = sync_channel(0);

        thread::spawn(move || {
            loop {
                if last_ping.is_none() || last_ping.as_ref().unwrap().elapsed() > keepalive_to {
                    let _ = self.socket.send_to(ping.as_ref(), self.server_addr);
                    last_ping = Some(Instant::now())
                }

                let mut buf = [0; 1500];
                let (n, addr) = match self.socket.recv_from(&mut buf) {
                    Ok((n, addr)) => (n, addr),
                    Err(_) => {
                        continue;
                    }
                };

                if addr == self.server_addr {
                    let resp = match Response::parse_from_bytes(&mut buf[..n]) {
                        Ok(resp) => resp,
                        Err(_) => {
                            continue;
                        }
                    };

                    if resp.get_id() != self.id {
                        continue;
                    }

                    match resp.cmd {
                        Some(RespCmd::Pong(_)) => {}
                        Some(RespCmd::Fsync(fsync)) => {
                            println!("{} call me", fsync.get_id());

                            last_peer_addr = match fsync.get_addr().parse() {
                                Ok(sa) => Some(sa),
                                _ => continue,
                            };

                            self.send_rsync(fsync.get_id(), fsync.get_addr());
                        }
                        _ => {}
                    };
                } else if last_peer_addr.is_some() && addr == *last_peer_addr.as_ref().unwrap() {
                    let req = match Request::parse_from_bytes(&mut buf[..n]) {
                        Ok(req) => req,
                        _ => {
                            //normal packet begin...we will drop it , but it is not unexpected.
                            tx.send((self.socket.try_clone().unwrap(), addr)).unwrap();
                            //we don't own the socket anymore.
                            self.rebind().unwrap();
                            last_ping = None;
                            continue;
                        }
                    };

                    match req.cmd.as_ref() {
                        Some(ReqCmd::Isync(isync)) => {
                            if isync.get_id() != self.id {
                                continue;
                            }

                            self.send_rsync(req.get_id(), addr);
                        }
                        _ => {}
                    }
                } else {
                    //unrelated packet
                }
            }
        });

        Ok(Acceptor { rx })
    }

    fn send_rsync<A: ToSocketAddrs>(&mut self, id: &str, addr: A) {
        let mut rsync = Rsync::new();
        rsync.set_id(self.id.to_string());

        let mut resp = Response::new();
        resp.set_id(id.to_string());
        resp.set_Rsync(rsync);

        let b = resp.write_to_bytes().unwrap();

        let _ = self.socket.send_to(b.as_ref(), addr);
    }
}

pub struct Acceptor<T> {
    rx: Receiver<T>,
}

impl<T> Acceptor<T> {
    pub fn accept(&mut self) -> Result<T> {
        self.rx.recv().map_err(|_| Error::new(Other, "accept fail"))
    }
}
