use protobuf::Message;
use std::collections::HashMap;
use std::io::Result;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Instant;

use crate::proto::{
    Fsync, Isync, Ping, Pong, Redirect, Request, Request_oneof_cmd as ReqCmd, Response,
    Response_oneof_cmd as RespCmd, Rsync,
};

#[derive(Copy, Clone)]
struct Client {
    addr: SocketAddr,
    last_ping: Instant,
}

impl Default for Client {
    fn default() -> Self {
        Client {
            addr: ([0, 0, 0, 0], 0).into(),
            last_ping: Instant::now(),
        }
    }
}

pub struct Server {
    socket: UdpSocket,
    clients: HashMap<String, Client>,
}

impl Server {
    pub fn new<A: ToSocketAddrs>(listen_addr: A) -> Result<Self> {
        let socket = UdpSocket::bind(listen_addr)?;

        Ok(Self {
            socket: socket,
            clients: Default::default(),
        })
    }

    pub fn run(mut self) -> Result<()> {
        let mut buf = [0; 1500];

        loop {
            if let Ok((size, addr)) = self.socket.recv_from(&mut buf) {
                if let Ok(req) = Request::parse_from_bytes(&buf[..size]) {
                    self.handle_request(req, addr);
                }
            }
        }
    }

    fn handle_request(&mut self, req: Request, addr: SocketAddr) {
        match req.cmd {
            Some(ReqCmd::Ping(ping)) => self.handle_ping(req.id, ping, addr),
            Some(ReqCmd::Isync(isync)) => self.handle_isync(req.id, isync, addr),
            Some(ReqCmd::Rsync(rsync)) => self.handle_rsync(req.id, rsync, addr),
            _ => {
                println!("unknown cmd {:?}", req)
            }
        };
    }

    fn send_response(&mut self, cmd: RespCmd, id: String, addr: SocketAddr) {
        let mut resp = Response::new();
        resp.set_id(id);
        resp.cmd = Some(cmd);

        let vec = resp.write_to_bytes().unwrap();
        let _ = self.socket.send_to(vec.as_ref(), addr);
    }

    fn handle_ping(&mut self, id: String, _ping: Ping, addr: SocketAddr) {
        let mut c = self
            .clients
            .entry(id.clone())
            .or_insert_with(|| Client::default());
        c.last_ping = Instant::now();
        c.addr = addr;

        self.send_response(RespCmd::Pong(Pong::new()), id, addr);
    }

    fn handle_isync(&mut self, id: String, isync: Isync, addr: SocketAddr) {
        let target_id = isync.get_id();
        if let Some(t) = self.clients.get(target_id).map(|t| *t) {
            let mut s = self.clients.entry(id.clone()).or_insert(Client::default());
            s.last_ping = Instant::now();
            s.addr = addr;

            let mut fsync = Fsync::new();
            fsync.set_id(id);
            fsync.set_addr(addr.to_string());
            self.send_response(RespCmd::Fsync(fsync), target_id.to_string(), t.addr);
        } else {
            println!("target id {} not found", target_id);
            let mut rdr = Redirect::new();
            rdr.set_id(target_id.to_string());
            self.send_response(RespCmd::Redirect(rdr), id.clone(), addr);
        }
    }

    fn handle_rsync(&mut self, id: String, rsync: Rsync, addr: SocketAddr) {
        let target_id = rsync.get_id();

        if let Some(c) = self.clients.get(target_id).map(|c| *c) {
            let mut rdr = Redirect::new();
            rdr.set_id(id.to_string());
            rdr.set_addr(addr.to_string());
            self.send_response(RespCmd::Redirect(rdr), target_id.to_string(), c.addr);
        } else {
            println!("rsync could not find target {}", target_id);
        }
    }
}
