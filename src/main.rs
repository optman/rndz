use std::io::Result;
use std::net::SocketAddr;
use structopt::StructOpt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task,
};

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
        Opt::Client(opt) => run_client(opt).await,
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

async fn run_client(opt: ClientOpt) -> Result<()> {
    if !opt.tcp {
        let mut c = udp::Client::new(&[&opt.server_addr], &opt.id, None, None)?;

        match opt.remote_peer {
            Some(peer) => {
                let s = c.connect(&peer)?;
                s.send(b"hello")?;
                let mut buf = [0; 1500];
                let (n, addr) = s.recv_from(&mut buf)?;
                log::debug!("receive {} bytes from {}", n, addr.to_string());
            }
            None => {
                let s = c.listen()?;
                let mut buf = [0; 1500];
                while let Ok((n, addr)) = s.recv_from(&mut buf) {
                    log::debug!("receive {} bytes from {}", n, addr.to_string());
                    let _ = s.send_to(&buf[..n], addr);
                }
            }
        }
    } else {
        let mut c = tcp::AsyncClient::new(&opt.server_addr, &opt.id, None)?;

        match opt.remote_peer {
            Some(peer) => {
                let mut stream = c.connect(&peer).await?;
                log::debug!("connect success");
                let _ = stream.write_all(b"hello").await;
                let mut buf = [0; 5];
                let n = stream.read(&mut buf).await.unwrap();
                log::debug!("read {} bytes", n);
            }
            None => {
                c.listen().await?;
                while let Ok((mut stream, addr)) = c.accept().await {
                    log::debug!("accept {}", addr.to_string());
                    task::spawn(async move {
                        let mut buf = [0; 5];
                        let n = stream.read(&mut buf).await.unwrap();
                        log::debug!("read {} bytes", n);
                        let _ = stream.write_all(b"hello").await;
                    });
                }
            }
        }
    }

    Ok(())
}
