use crate::proto::{
    Bye, Fsync, Isync, Ping, Pong, Redirect, Request, Request_oneof_cmd as ReqCmd, Response,
    Response_oneof_cmd as RespCmd,
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

pub struct Server {
    listener: TcpListener,
    peers: PeerMap,
    count: u64,
}

impl Server {
    pub async fn new<A: ToSocketAddrs>(listen_addr: A) -> Result<Self> {
        let listener = TcpListener::bind(listen_addr).await?;

        Ok(Self {
            listener: listener,
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
                let (tx, rx) = channel(1);
                let (r, w) = stream.split();
                let h = PeerHandler {
                    stream: w,
                    peers: peers,
                    peer_id: "".to_string(),
                    req_rx: rx,
                    req_tx: tx,
                    id: id,
                };

                h.handle_stream(r).await;
            });
        }

        Ok(())
    }
}

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

        if self.peer_id != "" {
            log::debug!("peer {} disconnect", self.peer_id);
        }
    }

    async fn read_reqs(mut r: ReadHalf<'a>, req_tx: Sender<Request>) -> Result<()> {
        loop {
            match timeout(Duration::from_secs(30), Self::read_req(&mut r)).await? {
                Ok(req) => req_tx
                    .send(req)
                    .await
                    .map_err(|_| Error::new(Other, "mpsc closed?")),
                _ => {
                    let mut bye = Request::new();
                    bye.set_Bye(Bye::new());
                    let _ = req_tx.send(bye);

                    Err(Error::new(Other, "byte"))
                }
            }?;
        }
    }

    async fn handle_cmds(&mut self) -> Result<()> {
        loop {
            match self.req_rx.recv().await {
                Some(req) => {
                    let src_id = req.get_id().to_string();
                    match req.cmd {
                        Some(ReqCmd::Ping(ping)) => self.handle_ping(src_id, ping).await,
                        Some(ReqCmd::Isync(isync)) => self.handle_isync(src_id, isync).await,
                        Some(ReqCmd::Fsync(fsync)) => self.handle_fsync(fsync).await,
                        Some(ReqCmd::Bye(_)) => Err(Error::new(Other, "bye")),
                        _ => Err(Error::new(Other, "uknown cmd")),
                    }
                }
                _ => Err(Error::new(Other, "bye")),
            }?;
        }
    }

    async fn read_req(stream: &mut ReadHalf<'a>) -> Result<Request> {
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf).await?;

        let size = u16::from_be_bytes(buf).into();
        if size > 1500 {
            Err(Error::new(Other, "invalid message"))?;
        }
        let mut buf = vec![0u8; size];
        stream.read_exact(&mut buf).await?;

        Request::parse_from_bytes(&mut buf).map_err(|_| Error::new(Other, "invalid message"))
    }

    async fn send_response(&mut self, cmd: RespCmd) -> Result<()> {
        let mut resp = Response::new();
        resp.cmd = Some(cmd);

        let vec = resp.write_to_bytes().unwrap();

        let _ = self
            .stream
            .write_all(&(vec.len() as u16).to_be_bytes())
            .await?;
        let _ = self.stream.write_all(&vec).await?;
        Ok(())
    }

    async fn handle_ping(&mut self, src_id: String, _ping: Ping) -> Result<()> {
        log::trace!("ping {}", src_id);

        self.peer_id = src_id;
        {
            let mut peers = self.peers.lock().unwrap();

            if match (*peers).get(&self.peer_id) {
                Some(p) => p.id != self.id,
                _ => false,
            } {
                peers.remove(&self.peer_id);
            }

            let mut p = peers
                .entry(self.peer_id.clone())
                .or_insert_with(|| PeerState {
                    id: self.id,
                    req_tx: self.req_tx.clone(),
                    last_ping: Instant::now(),
                    addr: self.stream.peer_addr().unwrap(),
                });

            p.last_ping = Instant::now();
        }

        self.send_response(RespCmd::Pong(Pong::new())).await
    }

    async fn handle_isync(&mut self, src_id: String, isync: Isync) -> Result<()> {
        let dst_id = isync.get_id();
        log::debug!("isync {} -> {}", src_id, dst_id);

        let mut rdr = Redirect::new();
        rdr.set_id(dst_id.to_string());

        if let Some((req_tx, freq)) = {
            let peers = self.peers.lock().unwrap();
            let p = match (*peers).get(dst_id) {
                Some(p) => Some(p),
                None => {
                    log::debug!("{} not found", dst_id);
                    None
                }
            };

            if let Some(p) = p {
                rdr.set_addr(p.addr.to_string());

                //forward
                let mut fsync = Fsync::new();
                fsync.set_id(src_id);
                fsync.set_addr(self.stream.peer_addr().unwrap().to_string());

                let mut freq = Request::new();

                freq.set_Fsync(fsync);

                Some((p.req_tx.clone(), freq))
            } else {
                None
            }
        } {
            let _ = req_tx.send(freq).await;
        }

        self.send_response(RespCmd::Redirect(rdr)).await
    }

    async fn handle_fsync(&mut self, fsync: Fsync) -> Result<()> {
        log::debug!("fsync {} -> {} ", fsync.get_id(), self.peer_id);
        self.send_response(RespCmd::Fsync(fsync)).await
    }
}
