# rndz

A simple rendezvous protocol implementation to help NAT traversal or hole punching.

The idea is simple, a rendezvous server to observe peers address and forward connection request. When seen both peers sent each other packet, the NAT device or firewall rule then allow the traffic through.

### pair two udp socket

client1
```
use rndz::Client;

let c1 = Client::new(rndz_server_addr, "c1")?;
let mut a = c1.listen()?;
while let Ok(socket, addr) = a.accept()?{
  //socket is ready to communicate with c2; a new listen socket will be automatic created, the port will changed.
}
```

client2
```
use rndz::Client;
let c2 = Client::new(rndz_server_addr, "c2")?;
let (socket, addr) = c.connect("c1")?;
//use socket to communicate with c1; the first packet will be drop.
```

### test

rndz server 
```
$ rndz server --listen-addr 0.0.0.0:8888    //if you want client communicate with ipv6, use [::]:8888
```

client1
```
$ rndz client --id c1 --server-addr rndz_server:8888 
```

client2
```
$ rndz client --id c2 --server-addr rndz_server:8888 --remote-peer c1
```

### see also 
[quic-tun](https://github.com/optman/quic-tun)

