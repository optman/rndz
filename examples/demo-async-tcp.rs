use rndz::tcp::AsyncClient;
use rndz::tcp::Server;
use std::error::Error;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::spawn,
    time::sleep,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let server_addr = "127.0.0.1:8888";

    {
        spawn(async move { Server::new(server_addr).await?.run().await });
    }

    let t = {
        spawn(async move {
            let mut c = AsyncClient::new(server_addr, "c1", None).unwrap();
            loop {
                match c.listen().await {
                    Ok(_) => {
                        let (mut s, _) = c.accept().await.unwrap();
                        s.write(b"hello").await.unwrap();
                        break;
                    }
                    _ => {
                        sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
        })
    };

    let mut c = AsyncClient::new(server_addr, "c2", None)?;
    let mut s = loop {
        match c.connect("c1").await {
            Ok(s) => break s,
            _ => sleep(Duration::from_secs(1)).await,
        }
    };

    let mut buf = [0; 5];
    s.read(&mut buf).await?;

    t.await?;

    Ok(())
}
