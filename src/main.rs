use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, Result};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc::{self};

use util::Conn;
use webrtc_ice::agent::agent_config::AgentConfig;
use webrtc_ice::agent::Agent;
use webrtc_ice::candidate::Candidate;

use webrtc_ice::candidate::candidate_base::unmarshal_candidate;
use webrtc_ice::network_type::NetworkType;
use webrtc_ice::state::ConnectionState;
use webrtc_ice::udp_mux::{UDPMuxDefault, UDPMuxParams};
use webrtc_ice::udp_network::UDPNetwork;

#[tokio::main]
async fn main() -> Result<()> {
    let (tx_auth, rx_auth) = mpsc::channel(1);
    // let (tx_transfer, mut rx_transfer) = mpsc::channel(1);
    let (tx_cand, mut rx_cand) = mpsc::channel(1);

    let control_arg = std::env::args().take(2).collect::<Vec<String>>();
    let _use_mux = false;
    let is_controlling = control_arg[1].parse::<bool>().unwrap();
    let _remote_port = 4000;
    let ice_agent = create_ice_agent(is_controlling).await.unwrap();

    // Get the local auth details and send to remote peer
    let (local_ufrag, local_pwd) = ice_agent.get_local_user_credentials().await;

    println!("Connecting");
    let connection = if is_controlling {
        TcpStream::connect("127.0.0.1:9001").await?
    } else {
        TcpStream::connect("172.30.29.1:9001").await?
    };
    ice_agent.on_candidate(handle_candidate(tx_cand)).await;
    let ice_agent2 = Arc::clone(&ice_agent);

    let (stream, sink) = connection.into_split();
    tokio::spawn(async move {
        stream_handler(
            local_ufrag,
            local_pwd,
            is_controlling,
            sink,
            stream,
            tx_auth,
            ice_agent2,
            rx_cand,
        )
        .await;
    });

    let (ice_done_tx, _ice_done_rx) = mpsc::channel::<()>(1);

    ice_agent
        .on_connection_state_change(Box::new(move |c: ConnectionState| {
            println!("ICE Connection State has changed: {}", c);
            if c == ConnectionState::Failed {
                let _ = ice_done_tx.try_send(());
            }
            Box::pin(async move {})
        }))
        .await;

    ice_agent.gather_candidates().await.unwrap();
    println!("Connecting...");
    let remote_credentials = tokio::spawn(remote_auth_handler(rx_auth, ice_agent, is_controlling))
        .await
        .unwrap();
    println!("Running");

    println!("Finished");
    Ok(())
}

async fn remote_auth_handler(
    mut rx_auth: mpsc::Receiver<String>,
    ice_agent: Arc<Agent>,
    is_controlling: bool,
) {
    loop {
        tokio::select! {
            remote_credentials_result = rx_auth.recv() => {
                let remote_credentials = remote_credentials_result.clone().unwrap().clone();

        let r = remote_credentials.split(":").take(2).collect::<Vec<&str>>();
        let (remote_ufrag, remote_pwd) = (r[0].to_string(), r[1].to_string());
        let (_cancel_tx, cancel_rx) = mpsc::channel(1);
        let _conn: Arc<dyn Conn + Send + Sync> = if is_controlling {
            std::thread::sleep(Duration::from_millis(100));
            println!("Dialing...");
            ice_agent
                .dial(cancel_rx, remote_ufrag, remote_pwd)
                .await
                .unwrap()
        } else {
            println!("Accepting...");
            ice_agent
                .accept(cancel_rx, remote_ufrag, remote_pwd)
                .await
                .unwrap()
        };
            }
        }
    }
}

async fn stream_handler(
    local_ufrag: String,
    local_pwd: String,
    is_controlling: bool,
    sink: tokio::net::tcp::OwnedWriteHalf,
    mut stream: tokio::net::tcp::OwnedReadHalf,
    tx_auth: mpsc::Sender<String>,
    ice_agent: Arc<Agent>,
    mut rx_cand: mpsc::Receiver<String>,
) {
    let mut buf = [0u8; 4096];
    let message_string = format!("{}:{}", local_ufrag, local_pwd);
    let message = message_string.as_bytes();

    if !is_controlling {
        sink.writable().await.unwrap();
        sink.try_write(message).unwrap();
    }
    loop {
        tokio::select! {
            candidate = rx_cand.recv() => {
                println!("{:?}", candidate);
                if let Some(s) = candidate {
                    if let Ok(c) = unmarshal_candidate(&s).await {
                        println!("add_remote_candidate: {}", c);
                        let c: Arc<dyn Candidate + Send + Sync> = Arc::new(c);
                        let _ = ice_agent.add_remote_candidate(&c).await;
                    } else {
                        println!("unmarshal_candidate error!");
                    }
                } else {
                    println!("REMOTE_CAND_CHANNEL done!");
                }
            },
            Ok(res) = stream.read(&mut buf) => {
                let bslice = std::str::from_utf8(&buf[..res]).unwrap().to_string();
                println!("Message: {}", bslice);
                // let message = std::str::from_utf8(&buf).unwrap();
                if bslice.contains(":") {
                    println!("Sending remote auth: {}", bslice);
                    tx_auth.send(bslice).await.unwrap();
                    if is_controlling {
                        // If controlling, send our auth credentials
                        sink.writable().await.unwrap();
                        sink.try_write(message).unwrap();
                    }
                }
            }
        }
    }
}

async fn create_ice_agent(_is_controlling: bool) -> Result<Arc<Agent>> {
    let local_port = 4000;

    let udp_network = {
        let udp_socket = UdpSocket::bind(("0.0.0.0", local_port)).await?;
        let udp_mux = UDPMuxDefault::new(UDPMuxParams::new(udp_socket));
        UDPNetwork::Muxed(udp_mux)
    };
    let agent = Agent::new(AgentConfig {
        network_types: vec![NetworkType::Udp4],
        udp_network,
        ..Default::default()
    })
    .await
    .unwrap();
    let ice_agent = Arc::new(agent);
    Ok(ice_agent)
}

fn handle_candidate<'a>(
    tx_cand: tokio::sync::mpsc::Sender<String>,
) -> Box<
    dyn FnMut(Option<Arc<dyn Candidate + Send + Sync>>) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
> {
    Box::new(move |c: Option<Arc<dyn Candidate + Send + Sync>>| {
        let tx_cand = tx_cand.clone();

        Box::pin(async move {
            let tx_cand = tx_cand.clone();
            if let Some(c) = c {
                println!("posting remoteCandidate with {}", c.marshal());

                tx_cand.send(c.marshal()).await.unwrap();
            }
        })
    })
}
