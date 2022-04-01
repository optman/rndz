use log;
use std::io::{Read, Result, Write};
use std::net::SocketAddr;
use std::thread;
use structopt::StructOpt;
use tokio::task;

use rndz::{tcp, udp};

#[derive(StructOpt, Debug)]
#[structopt(name = "rndz")]
enum Opt {
    Client(ClientOpt),
    Server(ServerOpt),
}

#[derive(StructOpt, Debug)]
struct ClientOpt {
    #[structopt(long = "id")]
    id: String,

    #[structopt(long = "server-addr")]
    server_addr: String,

    #[structopt(long = "remote-peer")]
    remote_peer: Option<String>,

    #[structopt(long = "tcp")]
    tcp: bool,
}

#[derive(StructOpt, Debug)]
struct ServerOpt {
    #[structopt(long = "listen-addr", default_value = "0.0.0.0:8888")]
    listen_addr: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt: Opt = StructOpt::from_args();

    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    match opt {
        Opt::Server(opt) => run_server(opt).await,
        Opt::Client(opt) => run_client(opt),
    }
}

async fn run_server(opt: ServerOpt) -> Result<()> {
    let s = udp::Server::new(opt.listen_addr)?;
    task::spawn_blocking(|| {
        s.run().unwrap();
    });

    let s = tcp::Server::new(opt.listen_addr).await?;
    s.run().await
}

fn run_client(opt: ClientOpt) -> Result<()> {
    if !opt.tcp {
        let mut c = udp::Client::new(&opt.server_addr, &opt.id, None)?;

        match opt.remote_peer {
            Some(peer) => {
                c.connect(&peer)?;
                let peer_addr = c.peer_addr().unwrap();
                c.as_socket().send_to(b"hello", peer_addr)?;
            }
            None => {
                c.listen()?;

                let mut buf = [0u8; 1500];
                while let Ok((n, addr)) = c.as_socket().recv_from(&mut buf) {
                    log::debug!("receive {} bytes from {}", n, addr.to_string());
                }
            }
        }
    } else {
        let mut c = tcp::Client::new(&opt.server_addr, &opt.id, None)?;

        match opt.remote_peer {
            Some(peer) => {
                let mut stream = c.connect(&peer)?;
                log::debug!("connect success");
                let mut buf = [0u8; 5];
                stream.read(&mut buf)?;
            }
            None => {
                c.listen()?;
                while let Ok((mut stream, addr)) = c.accept() {
                    log::debug!("accept {}", addr.to_string());
                    thread::spawn(move || {
                        let _ = stream.write_all(b"hello");
                    });
                }
            }
        }
    }

    Ok(())
}
