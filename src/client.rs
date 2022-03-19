use super::proto::rndz::{
    Isync, Ping, Request, Request_oneof_cmd as ReqCmd, Response, Response_oneof_cmd as RespCmd,
    Rsync,
};
use protobuf::Message;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};
use std::{
    io,
    io::{Error, ErrorKind},
};

pub struct Client {
    socket: UdpSocket,
    server_addr: SocketAddr,
    id: String,
}

impl Client {
    pub fn new<A: AsRef<str>>(socket: UdpSocket, server_addr: SocketAddr, id: A) -> Self {
        Self {
            socket: socket,
            server_addr: server_addr,
            id: id.as_ref().into(),
        }
    }

    fn new_req(&self) -> Request {
        let mut req = Request::new();
        req.set_id(self.id.clone());

        req
    }

    fn recv_resp(&mut self) -> io::Result<(Response, SocketAddr)> {
        let mut buf = [0; 1500];
        let (n, addr) = self.socket.recv_from(&mut buf)?;
        let resp = Response::parse_from_bytes(&mut buf[..n])?;
        Ok((resp, addr))
    }

    pub fn connect<A: AsRef<str>>(&mut self, target_id: A) -> io::Result<()> {
        let mut isync = Isync::new();
        isync.set_id(target_id.as_ref().into());

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
                            return Err(Error::new(ErrorKind::Other, "target not found"));
                        }
                    }
                    _ => {}
                }
            }
        }

        if peer_addr.is_none() {
            return Err(Error::from(ErrorKind::NotConnected));
        }

        let peer_addr = peer_addr.unwrap();

        println!("id {} addrs {}", target_id.as_ref(), peer_addr);

        for _ in 0..3 {
            let _ = self.socket.send_to(isync.as_ref(), peer_addr.as_str());

            if let Ok((resp, addr)) = self.recv_resp() {
                if resp.id != self.id || addr.to_string() != peer_addr {
                    continue;
                }
                match resp.cmd {
                    Some(RespCmd::Rsync(rsync)) => {
                        if rsync.id == target_id.as_ref() {
                            return Ok(());
                        }
                    }
                    _ => {}
                }
            }
        }

        Err(Error::from(ErrorKind::NotConnected))
    }

    pub fn listen(&mut self) -> io::Result<()> {
        let mut req = self.new_req();
        req.set_Ping(Ping::new());
        let ping = req.write_to_bytes()?;

        let mut last_ping: Option<Instant> = None;
        let keepalive_to = Duration::from_secs(10);
        self.socket.set_read_timeout(Some(keepalive_to))?;

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

                        self.send_rsync(fsync.get_id(), fsync.get_addr());
                    }
                    _ => {}
                };
            } else {
                let req = match Request::parse_from_bytes(&mut buf[..n]) {
                    Ok(req) => req,
                    _ => continue,
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
            }
        }
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
