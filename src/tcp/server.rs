use crate::proto::{
    Bye, Fsync, Isync, Ping, Pong, Redirect, Request, Request_oneof_cmd as ReqCmd, Response,
    Response_oneof_cmd as RespCmd, Rsync,
};
use log;
use protobuf::Message;
use std::collections::HashMap;
use std::io::{Error, ErrorKind::Other, Result};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{ReadHalf, WriteHalf},
        TcpListener, ToSocketAddrs,
    },
    select, task,
    time::timeout,
};

/// Tcp rendezvous server
///
/// keep traces of all peers, and forward connection request.
pub struct Server {
    listener: TcpListener,
    peers: PeerMap,
    count: u64,
}

impl Server {
    pub async fn new<A: ToSocketAddrs>(listen_addr: A) -> Result<Self> {
        let listener = TcpListener::bind(listen_addr).await?;

        Ok(Self {
            listener,
            peers: Default::default(),
            count: 0,
        })
    }

    fn next_id(&mut self) -> u64 {
        self.count += 1;
        self.count
    }

    pub async fn run(mut self) -> Result<()> {
        while let Ok((mut stream, _addr)) = self.listener.accept().await {
            let id = self.next_id();
            let peers = self.peers.clone();

            task::spawn(async move {
                let (tx, rx) = channel(10);
                let (r, w) = stream.split();
                let h = PeerHandler {
                    stream: w,
                    peers,
                    peer_id: "".to_string(),
                    req_rx: rx,
                    req_tx: tx,
                    id,
                };

                h.handle_stream(r).await;
            });
        }

        Ok(())
    }
}

#[derive(Clone)]
struct PeerState {
    id: u64,
    last_ping: Instant,
    req_tx: Sender<Request>,
    addr: SocketAddr,
}

type PeerMap = Arc<Mutex<HashMap<String, PeerState>>>;

struct PeerHandler<'a> {
    stream: WriteHalf<'a>,
    peers: PeerMap,
    peer_id: String,
    id: u64,
    req_tx: Sender<Request>,
    req_rx: Receiver<Request>,
}

impl<'a> PeerHandler<'a> {
    async fn handle_stream(mut self, r: ReadHalf<'a>) {
        let req_tx = self.req_tx.clone();

        select! {
            _ = Self::read_reqs(r, req_tx) => {},
            _ = self.handle_cmds()=> {},
        }

        let mut peers = self.peers.lock().unwrap();
        if let Some(p) = (*peers).get(&self.peer_id) {
            if p.id == self.id {
                peers.remove(&self.peer_id);
            }
        }

        if !self.peer_id.is_empty() {
            log::debug!("peer {} disconnect", self.peer_id);
        }
    }

    async fn read_reqs(mut r: ReadHalf<'a>, req_tx: Sender<Request>) -> Result<()> {
        loop {
            match timeout(Duration::from_secs(30), Self::read_req(&mut r)).await? {
                Ok(req) => req_tx
                    .send(req)
                    .await
                    .map_err(|_| Error::new(Other, "mpsc channel closed")),
                _ => {
                    let mut bye = Request::new();
                    bye.set_Bye(Bye::new());
                    let _ = req_tx.send(bye).await;

                    Err(Error::new(Other, "timeout reading request"))
                }
            }?;
        }
    }

    async fn handle_cmds(&mut self) -> Result<()> {
        while let Some(req) = self.req_rx.recv().await {
            let src_id = req.get_id().to_string();
            match req.cmd {
                Some(ReqCmd::Ping(ping)) => self.handle_ping(src_id, ping).await?,
                Some(ReqCmd::Isync(isync)) => self.handle_isync(src_id, isync).await?,
                Some(ReqCmd::Fsync(fsync)) => self.handle_fsync(fsync).await?,
                Some(ReqCmd::Rsync(rsync)) => self.handle_rsync(src_id, rsync).await?,
                Some(ReqCmd::Bye(_)) => return Err(Error::new(Other, "received bye command")),
                _ => return Err(Error::new(Other, "unknown command received")),
            };
        }

        Err(Error::new(Other, "cmd handler channel closed"))
    }

    async fn read_req(stream: &mut ReadHalf<'a>) -> Result<Request> {
        let mut buf = [0; 2];
        stream.read_exact(&mut buf).await?;

        let size = u16::from_be_bytes(buf).into();
        if size > 1500 {
            return Err(Error::new(Other, "invalid message size"));
        }
        let mut buf = vec![0; size];
        stream.read_exact(&mut buf).await?;

        Request::parse_from_bytes(&buf).map_err(|_| Error::new(Other, "failed to parse request"))
    }

    async fn send_response(&mut self, cmd: RespCmd) -> Result<()> {
        let mut resp = Response::new();
        resp.cmd = Some(cmd);

        let vec = resp.write_to_bytes().unwrap();

        self.stream
            .write_all(&(vec.len() as u16).to_be_bytes())
            .await?;
        self.stream.write_all(&vec).await?;
        Ok(())
    }

    fn insert_peerstate(&mut self, id: String) {
        self.peer_id = id;
        let mut peers = self.peers.lock().unwrap();

        if let Some(p) = peers.get(&self.peer_id) {
            if p.id != self.id {
                log::debug!("updating peer {}", self.peer_id);
                peers.remove(&self.peer_id);
            }
        }
        let p = peers
            .entry(self.peer_id.clone())
            .or_insert_with(|| PeerState {
                id: self.id,
                req_tx: self.req_tx.clone(),
                last_ping: Instant::now(),
                addr: self.stream.as_ref().peer_addr().unwrap(),
            });

        p.last_ping = Instant::now();
    }

    async fn handle_ping(&mut self, src_id: String, _ping: Ping) -> Result<()> {
        log::trace!("handled ping from {}", src_id);

        self.insert_peerstate(src_id);

        self.send_response(RespCmd::Pong(Pong::new())).await
    }

    async fn handle_isync(&mut self, src_id: String, isync: Isync) -> Result<()> {
        let dst_id = isync.get_id();
        log::debug!("handling isync from {} to {}", src_id, dst_id);

        let mut rdr = Redirect::new();
        rdr.set_id(dst_id.to_string());

        let peer_opt = self.peers.lock().unwrap().get(dst_id).cloned();
        if let Some(peer) = peer_opt {
            rdr.set_addr(peer.addr.to_string());

            // Forward sync request
            let mut fsync = Fsync::new();
            fsync.set_id(src_id.clone());
            fsync.set_addr(self.stream.as_ref().peer_addr().unwrap().to_string());
            let mut req = Request::new();
            req.set_Fsync(fsync);

            let _ = peer.req_tx.send(req).await;

            // Wait for reply (rsync)
            self.insert_peerstate(src_id);
            Ok(())
        } else {
            log::debug!("destination {} not found", dst_id);
            self.send_response(RespCmd::Redirect(rdr)).await
        }
    }

    async fn handle_fsync(&mut self, fsync: Fsync) -> Result<()> {
        log::debug!("handling fsync from {} to {}", fsync.get_id(), self.peer_id);
        self.send_response(RespCmd::Fsync(fsync)).await
    }

    async fn handle_rsync(&mut self, src_id: String, rsync: Rsync) -> Result<()> {
        let dst_id = rsync.get_id();
        log::debug!("handling rsync from {} to {}", src_id, dst_id);

        if dst_id == self.peer_id {
            let peer_opt = self.peers.lock().unwrap().get(&src_id).cloned();
            if let Some(peer) = peer_opt {
                let mut rdr = Redirect::new();
                rdr.set_id(src_id.to_string());
                rdr.set_addr(peer.addr.to_string());

                return self.send_response(RespCmd::Redirect(rdr)).await;
            } else {
                log::debug!("source {} not found", src_id);
                return Ok(());
            }
        }
        let peer_opt = self.peers.lock().unwrap().get(dst_id).cloned();
        if let Some(peer) = peer_opt {
            let mut req = Request::new();
            req.set_id(src_id);
            req.cmd = Some(ReqCmd::Rsync(rsync));

            let _ = peer.req_tx.send(req).await;
            Ok(())
        } else {
            log::debug!("destination {} not found", dst_id);
            Ok(())
        }
    }
}
