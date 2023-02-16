use crate::proto::{
    Bye, Isync, Ping, Request, Request_oneof_cmd as ReqCmd, Response,
    Response_oneof_cmd as RespCmd, Rsync,
};
use log;
use protobuf::Message;
use socket2::{Domain, Protocol, Socket, Type};
use std::io::{Error, ErrorKind::Other, Result};
use std::mem::MaybeUninit;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{
    atomic::{AtomicBool, Ordering::Relaxed},
    Arc, RwLock,
};
use std::thread::spawn;
use std::time::{Duration, Instant};

/// Udp socket builder
///
///
/// # example
/// ```no_run
/// use rndz::udp::Client;
///
/// let mut c1 = Client::new("rndz_server:1234", "c1", None).unwrap();
/// let s = c1.listen().unwrap();
/// let mut buf = [0u8, 10];
/// s.recv_from(&mut buf).unwrap();
/// ```
///
/// ```no_run
/// use rndz::udp::Client;
///
/// let mut c2 = Client::new("rndz_server:1234", "c2", None).unwrap();
/// let s= c2.connect("c1").unwrap();
/// s.send(b"hello").unwrap();
/// ```
pub struct Client {
    svr_sk: Option<Socket>,
    server_addr: SocketAddr,
    id: String,
    local_addr: SocketAddr,
    exit: Arc<AtomicBool>,
    last_pong: Arc<RwLock<Option<Instant>>>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.exit.store(true, Relaxed);
        self.drop_server_sk();
    }
}

impl Client {
    /// set rendezvous server, peer identity, local bind address.
    /// if no local address set, choose according server address type(ipv4 or ipv6).
    pub fn new(server_addr: &str, id: &str, local_addr: Option<SocketAddr>) -> Result<Self> {
        let svr_sk = Self::connect_server(server_addr, local_addr)?;
        Self::_new(svr_sk, id)
    }

    /// svr_sk is used to connect with rndz server
    pub fn new_with_socket(svr_addr: &str, id: &str, svr_sk: UdpSocket) -> Result<Self> {
        svr_sk.connect(svr_addr)?;
        let svr_sk: Socket = svr_sk.into();
        svr_sk.set_nonblocking(false)?;
        svr_sk.set_reuse_address(true)?;
        Self::_new(svr_sk, id)
    }

    fn _new(svr_sk: Socket, id: &str) -> Result<Self> {
        let local_addr = svr_sk.local_addr().unwrap().as_socket().unwrap();

        Ok(Self {
            server_addr: svr_sk.peer_addr().unwrap().as_socket().unwrap(),
            svr_sk: Some(svr_sk),
            id: id.into(),
            local_addr,
            exit: Default::default(),
            last_pong: Default::default(),
        })
    }

    /// local address
    pub fn local_addr(&self) -> Option<SocketAddr> {
        Some(self.local_addr)
    }

    /// last recv pong from server
    pub fn last_pong(&self) -> Option<Instant> {
        *self.last_pong.read().unwrap()
    }

    fn connect_server(server_addr: &str, local_addr: Option<SocketAddr>) -> Result<Socket> {
        let server_addr = server_addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| Error::new(Other, "no addr"))?;

        let local_addr = match local_addr {
            Some(addr) => addr,
            None => match server_addr {
                SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
                SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
            },
        };

        let domain = Domain::for_address(local_addr);

        let svr_sk = Self::create_socket(domain)?;
        svr_sk.bind(&local_addr.into())?;
        svr_sk.connect(&server_addr.into())?;

        Ok(svr_sk)
    }

    fn drop_server_sk(&mut self) {
        if let Some(mut s) = self.svr_sk.take() {
            Self::send_cmd(&mut s, &self.id, ReqCmd::Bye(Bye::new()), self.server_addr);
        };
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
        let mut buf = unsafe { MaybeUninit::<[MaybeUninit<u8>; 1500]>::uninit().assume_init() }; //MaybeUninit::uninit_array
        let n = self.svr_sk.as_ref().unwrap().recv(&mut buf)?;
        let buf = unsafe { &*(&buf as *const _ as *const [u8; 1500]) }; //MaybeUninit::slice_assume_init_ref
        let resp = Response::parse_from_bytes(&buf[..n])?;
        Ok(resp)
    }

    /// send rendezvous server a request to connect target peer.
    ///
    /// create a connected udp socket with peer stored in peer_sk field, use as_socket() to get it.
    ///
    pub fn connect(&mut self, target_id: &str) -> Result<UdpSocket> {
        if self.svr_sk.is_none() {
            //This is a reconnect
            self.svr_sk = Some(Self::connect_server(
                &self.server_addr.to_string(),
                Some(self.local_addr),
            )?);

            //From now on, there are two same port socket, WINDOWS will confuse!
            //Windows can't works well with two same port sockets.

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

            if let Ok(resp) = self.recv_resp() {
                if resp.get_id() != self.id {
                    continue;
                }

                if let Some(RespCmd::Redirect(rdr)) = resp.cmd {
                    if !rdr.addr.is_empty() {
                        peer_addr = Some(rdr.addr);
                        break;
                    } else {
                        return Err(Error::new(Other, "target not found"));
                    }
                }
            }
        }

        let peer_addr: SocketAddr = peer_addr
            .ok_or_else(|| Error::new(Other, "no response"))?
            .parse()
            .map_err(|_| Error::new(Other, "rndz server response invalid peer address"))?;

        //we don't need svr_sk any more, to prevent interferce with peer_sk on WINDOWS, we drop it.
        self.drop_server_sk();

        let domain = Domain::for_address(self.local_addr);
        let peer_sk = Self::create_socket(domain)?;
        peer_sk.bind(&self.local_addr.into())?;
        peer_sk.connect(&peer_addr.into())?;

        Ok(peer_sk.into())
    }

    /// keep ping rendezvous server, wait for peer connection request.
    ///
    /// when received `Fsync` request from server, attempt to send remote peer a packet
    /// this will open the firwall and nat rule for the peer.
    pub fn listen(&mut self) -> Result<UdpSocket> {
        #[cfg(windows)]
        log::warn!("WARNING: listen not works on WINDOWS!!!");

        let mut svr_sk = self.svr_sk.as_ref().unwrap().try_clone().unwrap();
        let myid = self.id.clone();
        let server_addr = self.server_addr;

        let exit = self.exit.clone();
        let last_pong = self.last_pong.clone();

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
                    unsafe { MaybeUninit::<[MaybeUninit<u8>; 1500]>::uninit().assume_init() }; //MaybeUninit::uninit_array
                let n = match svr_sk.recv(&mut buf) {
                    Ok(n) => n,
                    Err(_) => continue,
                };

                let buf = unsafe { &*(&buf as *const _ as *const [u8; 1500]) }; //MaybeUninit::slice_assume_init_ref

                let resp = match Response::parse_from_bytes(&buf[..n]) {
                    Ok(resp) => resp,
                    Err(_) => continue,
                };

                if resp.get_id() != myid {
                    continue;
                }

                match resp.cmd {
                    Some(RespCmd::Pong(_)) => {
                        *last_pong.write().unwrap() = Some(Instant::now());
                    }
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

        let domain = Domain::for_address(self.local_addr);
        let peer_sk = Self::create_socket(domain)?;
        peer_sk.bind(&self.local_addr.into())?;
        Ok(peer_sk.into())
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
