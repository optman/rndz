# rndz

A simple rendezvous protocol implementation to help with NAT traversal or hole punching.

The main idea is straightforward: using a rendezvous server to observe peers' addresses and forward connection requests. When both peers send packets to each other, the NAT device or firewall rule then allows the traffic through.

## Usage

### TCP Listen/Connect

#### Client 1

```rust
use rndz::tcp::Client;

let c1 = Client::new(rndz_server_addr, "c1", None)?;
c1.listen()?;
while let Ok(stream) = c1.accept()? {
    // Handle stream
}
```

#### Client 2

```rust
use rndz::tcp::Client;

let c2 = Client::new(rndz_server_addr, "c2", None)?;
let stream = c2.connect("c1")?;
```

### Pair Two UDP Sockets

#### Client 1

```rust
use rndz::udp::Client;

let c1 = Client::new(rndz_server_addr, "c1", None)?;
c1.listen()?;
c1.as_socket().recv_from(...)?
```

#### Client 2

```rust
use rndz::udp::Client;

let c2 = Client::new(rndz_server_addr, "c2", None)?;
c2.connect("c1")?;
c2.as_socket().send(b'hello')?
```

## Testing

### Rndz Server

```sh
$ rndz server --listen-addr 0.0.0.0:8888    # To allow clients to communicate with IPv6, use [::]:8888
```

### Client 1

```sh
$ rndz client --id c1 --server-addr rndz_server:8888
```

### Client 2

```sh
$ rndz client --id c2 --server-addr rndz_server:8888 --remote-peer c1
```

## Portability

Due to the reliance on socket options `SO_REUSEADDR` and `SO_REUSEPORT` [behavior](https://stackoverflow.com/questions/14388706/how-do-so-reuseaddr-and-so-reuseport-differ/14388707#14388707), and the [`connected` UDP socket](https://blog.cloudflare.com/everything-you-ever-wanted-to-know-about-udp-sockets-but-were-afraid-to-ask-part-1/), it does not work on all platforms.

Tests pass on Linux; however, `udp::Client::listen()` does not work on Windows.

## Used in Projects

- [quic-tun](https://github.com/optman/quic-tun): A QUIC-based port forward.
- [minivtun-rs](https://github.com/optman/minivtun-rs): A UDP-based VPN.
- [rndz-go](https://github.com/optman/rndz-go): Golang implementation of rndz.