use rndz::tcp::{Client, Server};
use std::error::Error;
use std::io::{Read, Write};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    let server_addr = "127.0.0.1:8888";

    {
        let server_addr = server_addr.clone();
        thread::spawn(move || Server::new(server_addr).unwrap().run().unwrap());
    }

    thread::spawn(move || loop {
        let server_addr = server_addr.clone();
        let mut c = Client::new(server_addr, "c1", None).unwrap();
        match c.listen() {
            Ok(_) => {
                while let Ok((mut s, _)) = c.accept() {
                    s.write(b"hello").unwrap();
                }
            }
            _ => {
                thread::sleep(Duration::from_secs(1));
                continue;
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

    let mut buf = [0u8; 5];
    s.read(&mut buf)?;

    Ok(())
}
