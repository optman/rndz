use rndz::tcp::{Client, Server};
use std::error::Error;
use std::io::{Read, Write};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    let server_addr = "127.0.0.1:8888";

    let rt = tokio::runtime::Runtime::new().unwrap();
    {
        let addr: std::net::SocketAddr = server_addr.parse()?;
        rt.spawn(async move { Server::new(addr, false).await.unwrap().run().await });
    }

    let t = thread::spawn(move || {
        let mut c = Client::new(server_addr, "c1", None).unwrap();
        loop {
            match c.listen() {
                Ok(_) => {
                    let (mut s, _) = c.accept().unwrap();
                    s.write_all(b"hello").unwrap();
                    break;
                }
                _ => {
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
    });

    let mut c = Client::new(server_addr, "c2", None).unwrap();
    let mut s = loop {
        match c.connect("c1") {
            Ok(s) => break s,
            _ => thread::sleep(Duration::from_secs(1)),
        }
    };

    let mut buf = [0; 5];
    s.read_exact(&mut buf)?;

    t.join().unwrap();

    Ok(())
}
