use crate::proto::{
    Bye, Fsync, Isync, Ping, Pong, Redirect, Request, Request_oneof_cmd as ReqCmd, Response,
    Response_oneof_cmd as RespCmd,
};
use protobuf::Message;
use std::collections::HashMap;
use std::io::{Error, ErrorKind::Other, Read, Result, Write};
use std::net::{Shutdown::Both, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub struct Server {
    listener: TcpListener,
    peers: PeerMap,
    count: u64,
}

impl Server {
    pub fn new<A: ToSocketAddrs>(listen_addr: A) -> Result<Self> {
        let listener = TcpListener::bind(listen_addr)?;

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

    pub fn run(mut self) -> Result<()> {
        while let Ok((stream, _addr)) = self.listener.accept() {
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(30)))
                .unwrap();

            let (tx, rx) = sync_channel(1);
            let h = PeerHandler {
                stream: stream,
                peers: self.peers.clone(),
                peer_id: "".to_string(),
                req_rx: rx,
                req_tx: tx,
                id: self.next_id(),
            };

            thread::spawn(move || h.handle_stream());
        }

        Ok(())
    }
}

struct PeerState {
    id: u64,
    last_ping: Instant,
    req_tx: SyncSender<Request>,
    addr: SocketAddr,
}

type PeerMap = Arc<Mutex<HashMap<String, PeerState>>>;

struct PeerHandler {
    stream: TcpStream,
    peers: PeerMap,
    peer_id: String,
    id: u64,
    req_tx: SyncSender<Request>,
    req_rx: Receiver<Request>,
}

impl PeerHandler {
    fn handle_stream(mut self) {
        let read_thread = {
            let mut stream = self.stream.try_clone().unwrap();
            let req_tx = self.req_tx.clone();
            thread::spawn(move || loop {
                match Self::read_req(&mut stream) {
                    Ok(req) => req_tx.send(req).unwrap(),
                    _ => {
                        let mut bye = Request::new();
                        bye.set_Bye(Bye::new());
                        let _ = req_tx.send(bye);
                        break;
                    }
                }
            })
        };

        while let Ok(req) = self.req_rx.recv() {
            let src_id = req.get_id().to_string();
            if let Err(_) = match req.cmd {
                Some(ReqCmd::Ping(ping)) => self.handle_ping(src_id, ping),
                Some(ReqCmd::Isync(isync)) => self.handle_isync(src_id, isync),
                Some(ReqCmd::Fsync(fsync)) => self.handle_fsync(fsync),
                Some(ReqCmd::Bye(_)) => break,
                _ => Ok(()),
            } {
                break;
            }
        }

        let _ = self.stream.shutdown(Both);

        let mut peers = self.peers.lock().unwrap();
        if let Some(p) = (*peers).get(&self.peer_id) {
            if p.id == self.id {
                peers.remove(&self.peer_id);
            }
        }

        let _ = read_thread.join();
    }

    fn read_req(stream: &mut TcpStream) -> Result<Request> {
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf)?;

        let size = u16::from_be_bytes(buf).into();
        if size > 1500 {
            Err(Error::new(Other, "invalid message"))?;
        }
        let mut buf = vec![0u8; size];
        stream.read_exact(&mut buf)?;

        Request::parse_from_bytes(&mut buf).map_err(|_| Error::new(Other, "invalid message"))
    }

    fn send_response(&mut self, cmd: RespCmd) -> Result<()> {
        let mut resp = Response::new();
        resp.cmd = Some(cmd);

        let vec = resp.write_to_bytes().unwrap();

        let _ = self.stream.write_all(&(vec.len() as u16).to_be_bytes())?;
        let _ = self.stream.write_all(&vec)?;
        Ok(())
    }

    fn handle_ping(&mut self, src_id: String, _ping: Ping) -> Result<()> {
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

        self.send_response(RespCmd::Pong(Pong::new()))
    }

    fn handle_isync(&mut self, src_id: String, isync: Isync) -> Result<()> {
        let dst_id = isync.get_id();

        let peers = self.peers.lock().unwrap();
        let p = match (*peers).get(dst_id) {
            Some(p) => Some(p),
            None => {
                println!("{} not found", dst_id);
                None
            }
        };

        let mut rdr = Redirect::new();
        rdr.set_id(dst_id.to_string());
        if let Some(p) = p {
            rdr.set_addr(p.addr.to_string());

            //forward
            let mut fsync = Fsync::new();
            fsync.set_id(src_id);
            fsync.set_addr(self.stream.peer_addr().unwrap().to_string());

            let mut freq = Request::new();
            freq.set_Fsync(fsync);

            let _ = p.req_tx.send(freq);
        }
        drop(peers);

        self.send_response(RespCmd::Redirect(rdr))
    }

    fn handle_fsync(&mut self, fsync: Fsync) -> Result<()> {
        self.send_response(RespCmd::Fsync(fsync))
    }
}
