use crate::proto::{
    Bye, Isync, Ping, Request, Request_oneof_cmd as ReqCmd, Response,
    Response_oneof_cmd as RespCmd, Rsync,
};
use log;
use protobuf::Message;
use socket2::{Domain, Protocol, Socket, Type};
use std::io::{Error, ErrorKind::Other, Result};
use std::mem::MaybeUninit;
use std::net::{Shutdown::Both, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{
    atomic::{AtomicBool, Ordering::Relaxed},
    Arc,
};
use std::thread::spawn;
use std::time::{Duration, Instant};

pub struct Client {
    svr_sk: Option<Socket>,
    server_addr: SocketAddr,
    id: String,
    peer_sk: Socket,
    local_addr: SocketAddr,
    peer_addr: Option<SocketAddr>,
    exit: Arc<AtomicBool>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.exit.store(true, Relaxed);
        self.drop_server_sk();
    }
}

impl Client {
    pub fn new(server_addr: &str, id: &str, local_addr: Option<SocketAddr>) -> Result<Self> {
        let svr_sk = Self::connect_server(server_addr, local_addr)?;

        let local_addr = svr_sk.local_addr().unwrap().as_socket().unwrap();
        let domain = match local_addr {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        };

        let peer_sk = Self::create_socket(domain)?;
        //DON'T bind now, for Windows can't works well with two same port sockets.

        Ok(Self {
            server_addr: svr_sk.peer_addr().unwrap().as_socket().unwrap(),
            svr_sk: Some(svr_sk),
            id: id.into(),
            peer_sk: peer_sk,
            local_addr: local_addr,
            peer_addr: None,
            exit: Default::default(),
        })
    }

    pub fn as_socket(&self) -> UdpSocket {
        self.peer_sk.try_clone().unwrap().into()
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        Some(self.local_addr)
    }

    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer_addr
    }

    fn connect_server(server_addr: &str, local_addr: Option<SocketAddr>) -> Result<Socket> {
        let server_addr = server_addr
            .to_socket_addrs()?
            .next()
            .ok_or(Error::new(Other, "no addr"))?;

        let local_addr = match local_addr {
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

        let svr_sk = Self::create_socket(domain)?;
        svr_sk.bind(&local_addr.into())?;
        svr_sk.connect(&server_addr.into())?;

        Ok(svr_sk)
    }

    fn drop_server_sk(&mut self) {
        self.svr_sk.take().map(|mut s| {
            Self::send_cmd(&mut s, &self.id, ReqCmd::Bye(Bye::new()), self.server_addr);
            s.shutdown(Both).unwrap();
        });
    }

    fn create_socket(domain: Domain) -> Result<Socket> {
        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        socket.set_reuse_address(true)?;

        Ok(socket)
    }

    fn new_req(myid: &str) -> Request {
        let mut req = Request::new();
        req.set_id(myid.to_string());

        req
    }

    fn recv_resp(&mut self) -> Result<Response> {
        let mut buf = unsafe { MaybeUninit::<[MaybeUninit<u8>; 1500]>::uninit().assume_init() };
        let n = self.svr_sk.as_ref().unwrap().recv(&mut buf)?;
        let buf = unsafe { (&buf as *const _ as *const [u8; 1500]).read() };
        let resp = Response::parse_from_bytes(&buf[..n])?;
        Ok(resp)
    }

    pub fn connect(&mut self, target_id: &str) -> Result<()> {
        if self.svr_sk.is_none() {
            //This is a reconnect
            self.svr_sk = Some(Self::connect_server(
                &self.server_addr.to_string(),
                Some(self.local_addr),
            )?);
            //From now on, there are two same port socket, WINDOWS will confuse!

            //NOTE: DON'T reconnect on WINDOWS!!!!
            #[cfg(windows)]
            log::warn!("WARNING: reconnect not works on WINDOWS!!!");
        }

        let mut isync = Isync::new();
        isync.set_id(target_id.into());

        let mut req = Self::new_req(&self.id);
        req.set_Isync(isync);

        let isync = req.write_to_bytes()?;

        self.svr_sk
            .as_ref()
            .unwrap()
            .set_read_timeout(Some(Duration::from_secs(10)))?;

        let mut peer_addr = None;
        for _ in 0..3 {
            self.svr_sk.as_ref().unwrap().send(isync.as_ref())?;

            match self.recv_resp() {
                Ok(resp) => {
                    if resp.get_id() != self.id {
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
                Err(_) => {}
            }
        }

        self.peer_addr = Some(
            peer_addr
                .ok_or(Error::new(Other, "no response"))?
                .parse()
                .map_err(|_| Error::new(Other, "rndz server response invalid peer address"))?,
        );

        //we don't need svr_sk any more, to prevent interferce with peer_sk on WINDOWS, we drop it.
        self.drop_server_sk();

        self.peer_sk.bind(&self.local_addr.into())?;
        self.peer_sk.connect(&self.peer_addr.unwrap().into())
    }

    pub fn listen(&mut self) -> Result<()> {
        #[cfg(windows)]
        log::warn!("WARNING: listen not works on WINDOWS!!!");

        let mut svr_sk = self.svr_sk.as_ref().unwrap().try_clone().unwrap();
        let myid = self.id.clone();
        let server_addr = self.server_addr.clone();

        let exit = self.exit.clone();

        spawn(move || {
            let mut req = Self::new_req(&myid);
            req.set_Ping(Ping::new());
            let ping = req.write_to_bytes().unwrap();

            let mut last_ping: Option<Instant> = None;
            let keepalive_to = Duration::from_secs(10);

            svr_sk.set_read_timeout(Some(keepalive_to)).unwrap();

            loop {
                if exit.load(Relaxed) {
                    break;
                }

                if last_ping.is_none() || last_ping.as_ref().unwrap().elapsed() > keepalive_to {
                    let _ = svr_sk.send(ping.as_ref());
                    last_ping = Some(Instant::now())
                }

                let mut buf =
                    unsafe { MaybeUninit::<[MaybeUninit<u8>; 1500]>::uninit().assume_init() };
                let n = match svr_sk.recv(&mut buf) {
                    Ok(n) => n,
                    Err(_) => continue,
                };

                let buf = unsafe { (&buf as *const _ as *const [u8; 1500]).read() };

                let resp = match Response::parse_from_bytes(&buf[..n]) {
                    Ok(resp) => resp,
                    Err(_) => continue,
                };

                if resp.get_id() != myid {
                    continue;
                }

                match resp.cmd {
                    Some(RespCmd::Pong(_)) => {}
                    Some(RespCmd::Fsync(fsync)) => {
                        log::debug!("fsync {}", fsync.get_id());

                        Self::send_rsync(&mut svr_sk, &myid, fsync.get_id(), server_addr);

                        match fsync.get_addr().parse() {
                            Ok(addr) => Self::send_rsync(&mut svr_sk, &myid, fsync.get_id(), addr),
                            _ => {
                                log::debug!("invalid fsync addr");
                                continue;
                            }
                        };
                    }
                    _ => {}
                };
            }
        });

        self.peer_sk.bind(&self.local_addr.into())
    }

    fn send_rsync(socket: &mut Socket, myid: &str, target_id: &str, addr: SocketAddr) {
        let mut rsync = Rsync::new();
        rsync.set_id(target_id.to_string());

        Self::send_cmd(socket, myid, ReqCmd::Rsync(rsync), addr);
    }

    fn send_cmd(socket: &mut Socket, myid: &str, cmd: ReqCmd, addr: SocketAddr) {
        let mut req = Request::new();
        req.set_id(myid.to_string());
        req.cmd = Some(cmd);

        let _ = socket.send_to(req.write_to_bytes().unwrap().as_ref(), &addr.into());
    }
}
