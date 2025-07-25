//! UDP client for rendezvous service.
//!
//! Use [`Listener`] to listen for incoming connections and [`Connector`] to initiate connections.
//!
//! # Example
//!
//! ## Listener
//!
//! ```no_run
//! use rndz::udp::client::Listener;
//!
//! let mut listener = Listener::new(&["rndz_server:1234"], "c1", None, None).unwrap();
//! let socket = listener.listen().unwrap();
//! let mut buf = [0; 10];
//! socket.recv_from(&mut buf).unwrap();
//! ```
//!
//! ## Connector
//!u
//! ```no_run
//! use rndz::udp::client::Connector;
//!
//! let connector = Connector::new(&["rndz_server:1234"], "c2", None, None).unwrap();
//! let socket = connector.connect("c1").unwrap();
//! socket.send(b"hello").unwrap();
//! ```
use crate::proto::rndz::{
    request::Cmd as ReqCmd, response::Cmd as RespCmd, Bye, Isync, Ping, Request, Response, Rsync,
};
use log;
use nix::poll::{poll, PollFd, PollFlags};
use protobuf::Message;
use rand::Rng;
use socket2::{Domain, Protocol, Socket, Type};
use std::io::{Error, ErrorKind::Other, Result};
use std::mem::MaybeUninit;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::sync::{
    atomic::{AtomicBool, Ordering::Relaxed},
    Arc, RwLock,
};
use std::thread::spawn;
use std::time::{Duration, Instant};
use std::vec;

type ServerPongStates = Vec<Option<Instant>>;

pub trait SocketConfigure {
    fn config_socket(&self, sk: RawFd) -> Result<()>;
}

// Common configuration shared between client types
struct CommonConfig {
    sk_cfg: Option<Box<dyn SocketConfigure>>,
    id: String,
    local_addr: Option<SocketAddr>,
    server_addrs: Vec<String>,
}

impl CommonConfig {
    fn new(
        server_addrs: &[&str],
        id: &str,
        local_addr: Option<SocketAddr>,
        sk_cfg: Option<Box<dyn SocketConfigure>>,
    ) -> Result<Self> {
        if server_addrs.is_empty() {
            return Err(Error::new(Other, "No server addresses provided"));
        }

        Ok(Self {
            sk_cfg,
            id: id.into(),
            local_addr,
            server_addrs: server_addrs.iter().map(|&s| s.to_string()).collect(),
        })
    }
}

/// A client that can connect to a specific peer.
pub struct Connector {
    cfg: CommonConfig,
}

impl Connector {
    /// Create a new connector to initiate a connection to a peer.
    /// If no local address set, it will be chosen according to server address type (IPv4 or IPv6).
    pub fn new(
        server_addrs: &[&str],
        id: &str,
        local_addr: Option<SocketAddr>,
        sk_cfg: Option<Box<dyn SocketConfigure>>,
    ) -> Result<Self> {
        let cfg = CommonConfig::new(server_addrs, id, local_addr, sk_cfg)?;
        Ok(Self { cfg })
    }

    /// Send rendezvous servers a request to connect to target peer.
    /// Try each server until a successful connection is established.
    ///
    /// Create a connected UDP socket with peer stored in peer_sk field, use `as_socket()` to get it.
    pub fn connect(&self, target_id: &str) -> Result<UdpSocket> {
        let mut isync = Isync::new();
        isync.id = target_id.to_string();

        let mut req = util::new_req(&self.cfg.id);
        req.set_Isync(isync);
        let isync = req.write_to_bytes()?;

        // Connect to all servers at once
        let (svr_sks, _) = util::connect_multiple_servers(
            &self.cfg.server_addrs,
            self.cfg.local_addr,
            self.cfg.sk_cfg.as_deref(),
        )?;

        let round_timeout = Duration::from_secs(5);
        let mut last_error = None;
        let mut failed_servers = vec![false; svr_sks.len()];

        // Set sockets to non-blocking mode
        for (sk, _) in &svr_sks {
            sk.set_nonblocking(true)?;
        }

        // Try 3 rounds of sync
        for _round in 0..3 {
            if failed_servers.iter().all(|&f| f) {
                log::debug!("All servers failed, stopping sync rounds");
                break;
            }

            log::debug!("Starting isync round {}", _round + 1);
            // Send sync request to all servers at start of round
            for (i, (sk, addr)) in svr_sks.iter().enumerate() {
                if !failed_servers[i] {
                    let _ = sk.send_to(isync.as_ref(), &(*addr).into());
                }
            }

            let round_start = Instant::now();
            while round_start.elapsed() < round_timeout {
                if failed_servers.iter().all(|&f| f) {
                    break;
                }
                match util::poll_sockets(&svr_sks, &failed_servers, 100) {
                    Ok(ready) => {
                        for (i, (sk, _)) in svr_sks.iter().enumerate() {
                            if !ready[i] {
                                continue;
                            }

                            let mut buf = unsafe {
                                MaybeUninit::<[MaybeUninit<u8>; 1500]>::uninit().assume_init()
                            };
                            match sk.recv(&mut buf) {
                                Ok(n) => {
                                    let buf = unsafe { &*(&buf as *const _ as *const [u8; 1500]) };
                                    if let Ok(resp) = Response::parse_from_bytes(&buf[..n]) {
                                        if resp.id != self.cfg.id {
                                            continue;
                                        }

                                        if let Some(RespCmd::Redirect(rdr)) = resp.cmd {
                                            if !rdr.addr.is_empty() {
                                                if let Ok(peer_addr) =
                                                    rdr.addr.parse::<SocketAddr>()
                                                {
                                                    let peer_sk = util::create_socket(
                                                        sk.local_addr()?.as_socket().unwrap(),
                                                        self.cfg.sk_cfg.as_deref(),
                                                    )?;
                                                    peer_sk.connect(&peer_addr.into())?;
                                                    return Ok(peer_sk.into());
                                                }
                                            } else {
                                                log::debug!("Server {} returned empty response, skipping in next rounds", i);
                                                last_error =
                                                    Some(Error::new(Other, "Target not found"));
                                                failed_servers[i] = true;
                                            }
                                        }
                                    }
                                }
                                Err(e) => last_error = Some(e),
                            }
                        }
                    }
                    Err(e) => {
                        last_error = Some(e);
                        break;
                    }
                }
            }
            log::debug!("Round {} timeout", _round + 1);
        }

        Err(last_error.unwrap_or_else(|| Error::new(Other, "No response from any server")))
    }
}

/// A client that listens for incoming connections from peers.
pub struct Listener {
    cfg: CommonConfig,
    svr_sks: Option<Vec<(Socket, SocketAddr)>>,
    exit: Arc<AtomicBool>,
    last_pong: Arc<RwLock<ServerPongStates>>,
}

impl Listener {
    /// Create a new listener to accept connections from peers.
    /// If no local address set, it will be chosen according to server address type (IPv4 or IPv6).
    pub fn new(
        server_addrs: &[&str],
        id: &str,
        local_addr: Option<SocketAddr>,
        sk_cfg: Option<Box<dyn SocketConfigure>>,
    ) -> Result<Self> {
        let cfg = CommonConfig::new(server_addrs, id, local_addr, sk_cfg)?;
        let addrs_len = cfg.server_addrs.len();
        Ok(Self {
            cfg,
            svr_sks: None,
            exit: Default::default(),
            last_pong: Arc::new(RwLock::new(vec![None; addrs_len])),
        })
    }

    /// Get the last received pong from any server
    pub fn last_pong(&self) -> Vec<Option<Instant>> {
        self.last_pong.read().unwrap().clone()
    }

    /// Keep pinging rendezvous servers, wait for peer connection request.
    ///
    /// When receiving `Fsync` request from any server, attempt to send remote peer a packet.
    /// This will open the firewall and NAT rule for the peer.
    pub fn listen(&mut self) -> Result<UdpSocket> {
        #[cfg(windows)]
        log::warn!("WARNING: listen not works on WINDOWS!!!");

        // Connect to servers if not already connected
        if self.svr_sks.is_none() {
            let (new_sks, local_addr) = util::connect_multiple_servers(
                &self.cfg.server_addrs,
                self.cfg.local_addr,
                self.cfg.sk_cfg.as_deref(),
            )?;
            self.svr_sks = Some(new_sks);
            self.cfg.local_addr = Some(local_addr);
        }

        let myid = self.cfg.id.clone();
        let exit = self.exit.clone();
        let last_pong = self.last_pong.clone();
        let keepalive_to = Duration::from_secs(10);

        // Prepare ping message once
        let mut req = util::new_req(&myid);
        req.set_Ping(Ping::new());
        let ping = req.write_to_bytes()?;

        let svr_sks = self.svr_sks.take().unwrap();

        // Set all sockets to non-blocking
        for (sk, _) in &svr_sks {
            sk.set_nonblocking(true)?;
        }

        // Create a socket for peer communication
        let peer_sk =
            util::create_socket(self.cfg.local_addr.unwrap(), self.cfg.sk_cfg.as_deref())?;

        // Spawn background thread for keepalive and return peer socket
        spawn(move || {
            // Add variation to initial ping times to avoid synchronized pings
            let mut ping_times: Vec<Option<Instant>> = vec![None; svr_sks.len()];
            let mut failed_servers = vec![false; svr_sks.len()];

            // Main event loop
            while !exit.load(Relaxed) {
                // Send pings if needed
                for (i, (sk, _addr)) in svr_sks.iter().enumerate() {
                    if ping_times[i].is_none()
                        || ping_times[i].as_ref().unwrap().elapsed() > keepalive_to
                    {
                        let _ = sk.send(ping.as_ref());
                        let variation = Duration::from_millis(
                            rand::thread_rng().gen_range(0..keepalive_to.as_millis() as u64 / 2),
                        );
                        ping_times[i] = Some(Instant::now() + variation);
                    }
                }

                // Poll all sockets
                match util::poll_sockets(&svr_sks, &failed_servers, 100) {
                    Ok(ready) => {
                        for (i, (sk, addr)) in svr_sks.iter().enumerate() {
                            if !ready[i] {
                                continue;
                            }

                            let mut buf = unsafe {
                                MaybeUninit::<[MaybeUninit<u8>; 1500]>::uninit().assume_init()
                            };
                            match sk.recv(&mut buf) {
                                Ok(n) => {
                                    let buf = unsafe { &*(&buf as *const _ as *const [u8; 1500]) };
                                    if let Ok(resp) = Response::parse_from_bytes(&buf[..n]) {
                                        if resp.id != myid {
                                            continue;
                                        }

                                        match resp.cmd {
                                            Some(RespCmd::Pong(_)) => {
                                                let mut last_pongs = last_pong.write().unwrap();
                                                last_pongs[i] = Some(Instant::now());
                                            }
                                            Some(RespCmd::Fsync(fsync)) => {
                                                log::debug!(
                                                    "fsync {} from server {}",
                                                    fsync.id,
                                                    addr
                                                );

                                                // Send Rsync to both the original server and the peer
                                                util::send_rsync(sk, &myid, &fsync.id, *addr);

                                                if let Ok(peer_addr) = fsync.addr.parse() {
                                                    util::send_rsync(
                                                        sk, &myid, &fsync.id, peer_addr,
                                                    );
                                                } else {
                                                    log::debug!("Invalid fsync address");
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                                Err(_) => {
                                    failed_servers[i] = true;
                                    continue;
                                }
                            }
                        }
                    }
                    Err(_) => continue,
                }
            }
        });

        Ok(peer_sk.into())
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.exit.store(true, Relaxed);
        if let Some(sks) = self.svr_sks.take() {
            for (s, addr) in sks {
                util::send_cmd(&s, &self.cfg.id, ReqCmd::Bye(Bye::new()), addr);
            }
        }
    }
}

// No replacement text needed as we've removed the Client enum entirely

// Helper functions for both Connector and Listener
pub(crate) mod util {
    use super::*;

    pub(crate) fn create_socket(
        addr: SocketAddr,
        sk_cfg: Option<&dyn SocketConfigure>,
    ) -> Result<Socket> {
        let domain = Domain::for_address(addr);
        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP)).unwrap();

        if let Some(cfg) = sk_cfg {
            cfg.config_socket(socket.as_raw_fd())?;
        }

        socket.set_reuse_address(true)?;
        socket.bind(&addr.into())?;

        Ok(socket)
    }

    // Create new request
    pub(crate) fn new_req(myid: &str) -> Request {
        let mut req = Request::new();
        req.id = myid.into();
        req
    }

    pub(crate) fn connect_multiple_servers(
        server_addrs: &[String],
        initial_addr: Option<SocketAddr>,
        sk_cfg: Option<&dyn SocketConfigure>,
    ) -> Result<(Vec<(Socket, SocketAddr)>, SocketAddr)> {
        let mut svr_sks = Vec::new();
        let mut local_addr = None;

        for server_addr in server_addrs {
            let svr_sk = connect_server(server_addr, local_addr.or(initial_addr), sk_cfg)?;
            let svr_addr = svr_sk.peer_addr().unwrap().as_socket().unwrap();

            if local_addr.is_none() {
                //combine the address from initial_addr and port from svr_sk.local_addr()
                local_addr = Some(if let Some(addr) = initial_addr {
                    let port = svr_sk.local_addr().unwrap().as_socket().unwrap().port();
                    match addr {
                        SocketAddr::V4(v4) => {
                            SocketAddr::V4(std::net::SocketAddrV4::new(*v4.ip(), port))
                        }
                        SocketAddr::V6(v6) => SocketAddr::V6(std::net::SocketAddrV6::new(
                            *v6.ip(),
                            port,
                            v6.flowinfo(),
                            v6.scope_id(),
                        )),
                    }
                } else {
                    svr_sk.local_addr().unwrap().as_socket().unwrap()
                });
            }

            svr_sks.push((svr_sk, svr_addr));
        }

        Ok((svr_sks, local_addr.unwrap()))
    }

    // Drop all server sockets
    // Connect to server
    fn connect_server(
        server_addr: &str,
        local_addr: Option<SocketAddr>,
        sk_cfg: Option<&dyn SocketConfigure>,
    ) -> Result<Socket> {
        let mut server_addr = server_addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| Error::new(Other, "No address found"))?;

        //if local_addr is ipv6, and server_addr is ipv4, convert it to ipv6
        if local_addr.map_or(false, |addr| addr.is_ipv6()) {
            if let SocketAddr::V4(v4) = server_addr {
                server_addr = SocketAddr::V6(std::net::SocketAddrV6::new(
                    v4.ip().to_ipv6_mapped(),
                    v4.port(),
                    0,
                    0,
                ));
            }
        }

        let local_addr = match local_addr {
            Some(addr) => addr,
            None => match server_addr {
                SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
                SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
            },
        };

        let svr_sk = create_socket(local_addr, sk_cfg)?;
        svr_sk.set_nonblocking(false)?;
        svr_sk.connect(&server_addr.into())?;

        log::debug!("Connected to server: {}", server_addr);

        Ok(svr_sk)
    }

    // Send rsync command
    pub(crate) fn send_rsync(socket: &Socket, myid: &str, target_id: &str, addr: SocketAddr) {
        let mut rsync = Rsync::new();
        rsync.id = target_id.into();

        send_cmd(socket, myid, ReqCmd::Rsync(rsync), addr);
    }

    // Send command
    pub(crate) fn send_cmd(socket: &Socket, myid: &str, cmd: ReqCmd, addr: SocketAddr) {
        let mut req = Request::new();
        req.id = myid.into();
        req.cmd = Some(cmd);

        let _ = socket.send_to(req.write_to_bytes().unwrap().as_ref(), &addr.into());
    }

    pub(crate) fn poll_sockets(
        sockets: &[(Socket, SocketAddr)],
        failed_servers: &[bool],
        timeout_ms: u16,
    ) -> Result<Vec<bool>> {
        let mut poll_fds: Vec<_> = sockets
            .iter()
            .enumerate()
            .filter(|(i, _)| !failed_servers[*i])
            .map(|(_, (sk, _))| PollFd::new(sk.as_fd(), PollFlags::POLLIN))
            .collect();

        if poll_fds.is_empty() {
            return Ok(vec![false; sockets.len()]);
        }

        match poll(&mut poll_fds, timeout_ms) {
            Ok(n) if n > 0 => {
                let mut ready = vec![false; sockets.len()];
                let mut poll_idx = 0;

                for (i, _) in sockets.iter().enumerate() {
                    if failed_servers[i] {
                        continue;
                    }

                    if let Some(events) = poll_fds[poll_idx].revents() {
                        ready[i] = events.contains(PollFlags::POLLIN);
                    }
                    poll_idx += 1;
                }

                Ok(ready)
            }
            Ok(_) => Ok(vec![false; sockets.len()]),
            Err(e) => Err(Error::new(Other, e.to_string())),
        }
    }
}
