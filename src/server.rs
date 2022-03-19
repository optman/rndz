use protobuf::Message;
use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Instant;

use super::proto::rndz::{
    Fsync, Isync, Ping, Pong, Redirect, Request, Request_oneof_cmd as ReqCmd, Response,
    Response_oneof_cmd as RespCmd,
};

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
    pub fn new<A: ToSocketAddrs>(listen_addr: A) -> io::Result<Self> {
        let socket = UdpSocket::bind(listen_addr)?;

        Ok(Self {
            socket: socket,
            clients: Default::default(),
        })
    }

    pub fn run(mut self) -> io::Result<()> {
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
            _ => {
                println!("unknown cmd {:?}", req)
            }
        };
    }

    fn send_response<A: AsRef<str>>(&mut self, cmd: RespCmd, id: A, addr: SocketAddr) {
        let mut resp = Response::new();
        resp.set_id(id.as_ref().to_string());
        resp.cmd = Some(cmd);

        let vec = resp.write_to_bytes().unwrap();
        let _ = self.socket.send_to(vec.as_ref(), addr);
    }

    fn handle_ping(&mut self, id: String, _ping: Ping, addr: SocketAddr) {
        let mut c = self.clients.entry(id.clone()).or_insert_with(|| {
            println!("new client {}", id);
            Client::default()
        });
        c.last_ping = Instant::now();
        c.addr = addr;

        self.send_response(RespCmd::Pong(Pong::new()), id, addr);
    }

    fn handle_isync(&mut self, id: String, isync: Isync, addr: SocketAddr) {
        let target_id = isync.get_id();
        println!("{} want connect {}", id, target_id);

        let mut rdr = Redirect::new();
        rdr.set_id(target_id.to_string());

        let (found, target_addr) = if let Some(c) = self.clients.get(target_id) {
            rdr.set_addr(c.addr.to_string());
            (true, c.addr)
        } else {
            println!("target id {} not found", target_id);
            (false, ([0, 0, 0, 0], 0).into())
        };

        self.send_response(RespCmd::Redirect(rdr), id.as_str(), addr);

        if found {
            let mut fsync = Fsync::new();
            fsync.set_id(id);
            fsync.set_addr(addr.to_string());
            self.send_response(RespCmd::Fsync(fsync), target_id, target_addr);
        }
    }
}
